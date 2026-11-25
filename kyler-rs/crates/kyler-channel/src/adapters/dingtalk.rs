use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use futures::{SinkExt, StreamExt};
use reqwest::Url;
use serde_json::{Value, json};
use tokio::{sync::Mutex, time::timeout};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

use crate::{ChannelStore, Incoming, Settings, adapter::client};

const TOPIC: &str = "/v1.0/im/bot/messages/get";

pub struct DingTalk {
    pub(crate) app: String,
    pub(crate) corp: String,
    secret: String,
    robot: String,
    http: reqwest::Client,
    token: Mutex<Option<(String, Instant)>>,
}

impl DingTalk {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        Ok(Self {
            app: settings.required("KYLER_DINGTALK_APP_KEY")?,
            corp: settings.required("KYLER_DINGTALK_CORP_ID")?,
            secret: settings.required("KYLER_DINGTALK_APP_SECRET")?,
            robot: settings.required("KYLER_DINGTALK_ROBOT_CODE")?,
            http: client()?,
            token: Mutex::new(None),
        })
    }

    pub async fn listen(&self, store: &ChannelStore) -> Result<()> {
        let response = self
            .http
            .post("https://api.dingtalk.com/v1.0/gateway/connections/open")
            .json(
                &json!({"clientId":self.app,"clientSecret":self.secret,"ua":"Kyler",
                "subscriptions":[{"type":"CALLBACK","topic":TOPIC}]}),
            )
            .send()
            .await?
            .error_for_status()?;
        let data: Value = response.json().await?;
        let endpoint = data["endpoint"]
            .as_str()
            .context("Missing DingTalk stream endpoint")?;
        let ticket = data["ticket"]
            .as_str()
            .context("Missing DingTalk stream ticket")?;
        let endpoint = stream_url(endpoint, ticket)?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(256 * 1024))
            .max_frame_size(Some(256 * 1024));
        let (mut socket, _) = timeout(
            Duration::from_secs(15),
            connect_async_with_config(endpoint.as_str(), Some(config), false),
        )
        .await??;
        loop {
            let frame = match timeout(Duration::from_secs(120), socket.next()).await {
                Ok(Some(frame)) => frame?,
                Ok(None) => return Ok(()),
                Err(_) => {
                    timeout(
                        Duration::from_secs(10),
                        socket.send(Message::Ping(Vec::new().into())),
                    )
                    .await??;
                    timeout(Duration::from_secs(10), socket.next())
                        .await?
                        .context("DingTalk stream closed during keepalive")??
                }
            };
            let text = match frame {
                Message::Text(text) => text,
                Message::Close(_) => return Ok(()),
                Message::Ping(_) => {
                    // Tungstenite queues the matching pong while reading the ping.
                    timeout(Duration::from_secs(10), socket.flush()).await??;
                    continue;
                }
                _ => continue,
            };
            let frame: Value = serde_json::from_str(&text)?;
            let (ack, incoming, disconnect) = decode_frame(&self.corp, &self.robot, &frame)?;
            if let Some(message) = incoming {
                store.ingest(vec![message]).await?;
            }
            // Do not ACK a message before its queue transaction has committed.
            timeout(
                Duration::from_secs(10),
                socket.send(Message::Text(ack.to_string().into())),
            )
            .await??;
            if disconnect {
                return Ok(());
            }
        }
    }

    async fn access_token(&self) -> Result<String, Option<u16>> {
        let mut cache = self.token.lock().await;
        if let Some((token, until)) = &*cache
            && *until > Instant::now()
        {
            return Ok(token.clone());
        }
        let response = self
            .http
            .post("https://api.dingtalk.com/v1.0/oauth2/accessToken")
            .json(&json!({"appKey":self.app,"appSecret":self.secret}))
            .send()
            .await
            .map_err(|_| None)?;
        if !response.status().is_success() {
            return Err(Some(response.status().as_u16()));
        }
        let data: Value = response.json().await.map_err(|_| None)?;
        let token = data["accessToken"].as_str().ok_or(None)?.to_owned();
        let expiry = data["expireIn"]
            .as_u64()
            .unwrap_or_default()
            .min(7200)
            .saturating_sub(60);
        *cache = Some((token.clone(), Instant::now() + Duration::from_secs(expiry)));
        Ok(token)
    }

    pub(crate) async fn send(&self, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let user = chat
            .strip_prefix("dt_")
            .filter(|id| staff_id(id))
            .ok_or(Some(400))?;
        let token = self.access_token().await?;
        let response = self
            .http
            .post("https://api.dingtalk.com/v1.0/robot/oToMessages/batchSend")
            .header("x-acs-dingtalk-access-token", token)
            .json(
                &json!({"robotCode":self.robot,"userIds":[user],"msgKey":"sampleText",
                "msgParam":json!({"content":text}).to_string()}),
            )
            .send()
            .await
            .map_err(|_| None)?;
        let status = response.status();
        if status.as_u16() == 401 {
            *self.token.lock().await = None;
        }
        if !status.is_success() {
            return Err(Some(status.as_u16()));
        }
        let data: Value = response.json().await.map_err(|_| None)?;
        if data["processQueryKey"].as_str().is_some() {
            Ok(())
        } else {
            Err(None)
        }
    }
}

fn stream_url(endpoint: &str, ticket: &str) -> Result<Url> {
    let mut url =
        Url::parse(endpoint).map_err(|_| anyhow::anyhow!("Invalid DingTalk stream URL"))?;
    ensure!(
        url.scheme() == "wss"
            && url
                .host_str()
                .is_some_and(|host| host == "dingtalk.com" || host.ends_with(".dingtalk.com"))
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && url.query().is_none()
            && !ticket.is_empty(),
        "Unexpected DingTalk stream endpoint"
    );
    url.query_pairs_mut().append_pair("ticket", ticket);
    Ok(url)
}

fn staff_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 120
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn decode_frame(corp: &str, robot: &str, frame: &Value) -> Result<(Value, Option<Incoming>, bool)> {
    let headers = &frame["headers"];
    let message_id = headers["messageId"]
        .as_str()
        .context("Missing DingTalk frame ID")?;
    let topic = headers["topic"].as_str().unwrap_or_default();
    let mut ack = json!({"code":200,"headers":{"messageId":message_id,"contentType":"application/json"},"message":"OK","data":"{}"});
    let disconnect = frame["type"] == "SYSTEM" && topic == "disconnect";
    if frame["type"] == "SYSTEM" && topic == "ping" {
        ack["data"] = frame["data"].clone();
    }
    if frame["type"] != "CALLBACK" || topic != TOPIC {
        return Ok((ack, None, disconnect));
    }
    let event: Value = serde_json::from_str(
        frame["data"]
            .as_str()
            .context("Missing DingTalk callback data")?,
    )?;
    let incoming = match (
        event["senderStaffId"].as_str(),
        event["msgId"].as_str(),
        event["text"]["content"].as_str(),
    ) {
        (Some(staff), Some(id), Some(text))
            if event["conversationType"] == "1"
                && event["msgtype"] == "text"
                && event["senderCorpId"] == corp
                && event["chatbotCorpId"] == corp
                && event["robotCode"] == robot
                && staff_id(staff)
                && !id.is_empty()
                && !text.trim().is_empty() =>
        {
            Some(Incoming {
                attachments: Vec::new(),
                update: format!("dt_{id}"),
                chat: format!("dt_{staff}"),
                text: text.into(),
            })
        }
        _ => None,
    };
    Ok((ack, incoming, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_configured_enterprise_private_robot_messages() -> Result<()> {
        for (kind, sender, accepted) in [
            ("1", "corp", true),
            ("2", "corp", false),
            ("1", "other", false),
        ] {
            let frame = json!({"type":"CALLBACK","headers":{"topic":TOPIC,"messageId":"frame-1"},"data":
                json!({"conversationType":kind,"msgtype":"text","senderCorpId":sender,"chatbotCorpId":"corp",
                    "robotCode":"robot","senderStaffId":"0012","msgId":"event-1","text":{"content":"hello"}}).to_string()});
            let (ack, incoming, _) = decode_frame("corp", "robot", &frame)?;
            assert_eq!(ack["headers"]["messageId"], "frame-1");
            assert_eq!(
                incoming.as_ref().map(|item| item.chat.as_str()),
                accepted.then_some("dt_0012")
            );
        }
        assert!(stream_url("wss://example.com/stream", "test").is_err());
        assert!(
            stream_url("wss://test.dingtalk.com/stream", "a&b")?
                .as_str()
                .ends_with("ticket=a%26b")
        );
        Ok(())
    }
}
