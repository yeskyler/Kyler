use std::time::Duration;

use anyhow::{Context, Result, ensure};
use futures::{SinkExt, StreamExt};
use reqwest::Url;
use serde_json::{Value, json};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

use crate::{ChannelStore, Incoming, Settings, adapter::client};

pub struct HomeAssistant {
    http: reqwest::Client,
    base: Url,
    token: String,
    user: String,
}

impl HomeAssistant {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let mut base = Url::parse(&settings.required("KYLER_HOME_ASSISTANT_URL")?)
            .map_err(|_| anyhow::anyhow!("Invalid Home Assistant URL"))?;
        ensure!(
            base.scheme() == "https"
                && base.host().is_some()
                && base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none(),
            "Home Assistant URL must be HTTPS without credentials, query, or fragment"
        );
        if !base.path().ends_with('/') {
            base.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("Invalid Home Assistant base path"))?
                .push("");
        }
        let user = settings.required("KYLER_HOME_ASSISTANT_USER_ID")?;
        ensure!(
            user.len() == 32 && user.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Invalid Home Assistant user ID"
        );
        Ok(Self {
            http: client()?,
            base,
            token: settings.required("KYLER_HOME_ASSISTANT_TOKEN")?,
            user,
        })
    }

    pub(crate) fn namespace(&self) -> String {
        format!("home_assistant:{}:{}", self.base, self.user)
    }

    pub async fn listen(&self, store: &ChannelStore) -> Result<()> {
        let mut url = self.base.join("api/websocket")?;
        url.set_scheme("wss")
            .map_err(|_| anyhow::anyhow!("Invalid Home Assistant WebSocket URL"))?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(256 * 1024))
            .max_frame_size(Some(256 * 1024));
        let (mut socket, _) = timeout(
            Duration::from_secs(15),
            connect_async_with_config(url.as_str(), Some(config), false),
        )
        .await??;
        let first = timeout(Duration::from_secs(10), socket.next())
            .await?
            .context("Home Assistant closed the socket")??;
        let first: Value = serde_json::from_str(first.to_text()?)?;
        ensure!(
            first["type"] == "auth_required",
            "Home Assistant did not request authentication"
        );
        let auth = json!({"type":"auth","access_token":self.token});
        timeout(
            Duration::from_secs(10),
            socket.send(Message::Text(auth.to_string().into())),
        )
        .await??;
        let result = timeout(Duration::from_secs(10), socket.next())
            .await?
            .context("Home Assistant closed during authentication")??;
        let result: Value = serde_json::from_str(result.to_text()?)?;
        ensure!(
            result["type"] == "auth_ok",
            "Home Assistant authentication failed"
        );

        let subscribe = json!({"id":1,"type":"subscribe_events","event_type":"kyler_prompt"});
        timeout(
            Duration::from_secs(10),
            socket.send(Message::Text(subscribe.to_string().into())),
        )
        .await??;
        let mut subscribed = false;
        let mut ping_id = 1u64;
        loop {
            let idle = if subscribed { 60 } else { 10 };
            let frame = match timeout(Duration::from_secs(idle), socket.next()).await {
                Ok(Some(frame)) => frame?,
                Ok(None) => return Ok(()),
                Err(_) if subscribed => {
                    ping_id += 1;
                    let ping = json!({"id":ping_id,"type":"ping"});
                    timeout(
                        Duration::from_secs(10),
                        socket.send(Message::Text(ping.to_string().into())),
                    )
                    .await??;
                    timeout(Duration::from_secs(10), socket.next())
                        .await?
                        .context("Home Assistant socket closed")??
                }
                Err(_) => anyhow::bail!("Home Assistant subscription timed out"),
            };
            let value: Value = match frame {
                Message::Text(text) => serde_json::from_str(&text)?,
                Message::Close(_) => return Ok(()),
                Message::Ping(_) => {
                    timeout(Duration::from_secs(10), socket.flush()).await??;
                    continue;
                }
                _ => continue,
            };
            if value["id"] == 1 && value["type"] == "result" {
                ensure!(
                    value["success"] == true,
                    "Home Assistant subscription failed"
                );
                subscribed = true;
            }
            if subscribed && let Some(message) = incoming(&self.user, &value) {
                store.ingest(vec![message]).await?;
            }
        }
    }

    pub(crate) async fn send(&self, id: i64, chat: &str, text: &str) -> Result<(), Option<u16>> {
        if chat.strip_prefix("ha_") != Some(self.user.as_str()) {
            return Err(Some(403));
        }
        let url = self
            .base
            .join("api/events/kyler_reply")
            .map_err(|_| Some(400))?;
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .json(&json!({"id":id.to_string(),"user_id":self.user,"text":text}))
            .send()
            .await
            .map_err(|_| None)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Some(response.status().as_u16()))
        }
    }
}

fn incoming(user: &str, value: &Value) -> Option<Incoming> {
    let event = &value["event"];
    if value["id"] != 1
        || value["type"] != "event"
        || event["event_type"] != "kyler_prompt"
        || event["context"]["user_id"] != user
    {
        return None;
    }
    let context = event["context"]["id"].as_str()?;
    let text = event["data"]["text"].as_str()?;
    if context.is_empty()
        || context.len() > 128
        || !context.bytes().all(|byte| byte.is_ascii_alphanumeric())
        || text.trim().is_empty()
    {
        return None;
    }
    Some(Incoming {
        attachments: Vec::new(),
        update: format!("ha_{context}"),
        chat: format!("ha_{user}"),
        text: text.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_origin_uses_home_assistant_context_not_payload_user() {
        let mut event = json!({"id":1,"type":"event","event":{"event_type":"kyler_prompt",
            "context":{"id":"01ABC","user_id":"other"},"data":{"text":"hello","user_id":"allowed"}}});
        assert!(incoming("allowed", &event).is_none());
        event["event"]["context"]["user_id"] = json!("allowed");
        assert_eq!(incoming("allowed", &event).unwrap().chat, "ha_allowed");
        event["event"]["event_type"] = json!("kyler_reply");
        assert!(incoming("allowed", &event).is_none());
    }
}
