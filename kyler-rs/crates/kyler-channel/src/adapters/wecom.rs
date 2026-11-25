use std::time::Duration;

use anyhow::{Result, ensure};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex, mpsc, oneshot},
    time::{Instant, interval_at, timeout},
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use uuid::Uuid;

use crate::{ChannelStore, Incoming, Settings};

struct Delivery {
    chat: String,
    text: String,
    result: oneshot::Sender<Result<(), Option<u16>>>,
}

pub struct WeCom {
    pub(crate) bot: String,
    secret: String,
    outbound: Mutex<Option<mpsc::Sender<Delivery>>>,
}

impl WeCom {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        Ok(Self {
            bot: settings.required("KYLER_WECOM_BOT_ID")?,
            secret: settings.required("KYLER_WECOM_BOT_SECRET")?,
            outbound: Mutex::new(None),
        })
    }

    pub async fn listen(&self, store: &ChannelStore) -> Result<()> {
        let config = WebSocketConfig::default()
            .max_message_size(Some(256 * 1024))
            .max_frame_size(Some(256 * 1024));
        let (mut socket, _) = timeout(
            Duration::from_secs(15),
            connect_async_with_config("wss://openws.work.weixin.qq.com", Some(config), false),
        )
        .await??;
        let id = Uuid::new_v4().to_string();
        let subscribe = frame(
            "aibot_subscribe",
            &id,
            json!({"bot_id":self.bot,"secret":self.secret}),
        );
        timeout(Duration::from_secs(10), socket.send(subscribe)).await??;
        timeout(Duration::from_secs(10), async {
            while let Some(message) = socket.next().await {
                match message? {
                    Message::Text(text) => {
                        let ack: Value = serde_json::from_str(&text)?;
                        if ack["headers"]["req_id"] == id {
                            ensure!(ack["errcode"] == 0, "WeCom subscription was rejected");
                            return Ok(());
                        }
                    }
                    Message::Ping(_) => socket.flush().await?,
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            anyhow::bail!("WeCom closed during authentication")
        })
        .await??;

        let (sender, mut queue) = mpsc::channel(1);
        *self.outbound.lock().await = Some(sender);
        let heartbeat = Duration::from_secs(25);
        let mut timer = interval_at(Instant::now() + heartbeat, heartbeat);
        let mut heartbeat_id = None;
        let mut pending: Option<(String, Delivery)> = None;
        loop {
            if pending
                .as_ref()
                .is_some_and(|(_, delivery)| delivery.result.is_closed())
            {
                pending = None;
            }
            tokio::select! {
                _ = timer.tick() => {
                    ensure!(heartbeat_id.is_none(), "WeCom missed its heartbeat acknowledgement");
                    let id = Uuid::new_v4().to_string();
                    timeout(Duration::from_secs(10), socket.send(frame("ping", &id, Value::Null))).await??;
                    heartbeat_id = Some(id);
                }
                Some(delivery) = queue.recv(), if pending.is_none() => {
                    if delivery.result.is_closed() { continue; }
                    let id = Uuid::new_v4().to_string();
                    let command = frame("aibot_send_msg", &id, json!({
                        "chatid":delivery.chat,"msgtype":"markdown","markdown":{"content":delivery.text}
                    }));
                    timeout(Duration::from_secs(10),socket.send(command)).await??;
                    pending = Some((id, delivery));
                }
                message = socket.next() => {
                    let Some(message) = message else { return Ok(()); };
                    let text = match message? {
                        Message::Text(text) => text,
                        Message::Close(_) => return Ok(()),
                        Message::Ping(_) => {
                            timeout(Duration::from_secs(10),socket.flush()).await??;
                            continue;
                        }
                        _ => continue,
                    };
                    let event: Value = serde_json::from_str(&text)?;
                    if let Some(message) = incoming(&self.bot, &event) {
                        store.ingest(vec![message]).await?;
                    }
                    if event["cmd"] == "aibot_event_callback" && event["body"]["event"]["eventtype"] == "disconnected_event" {
                        return Ok(());
                    }
                    let ack = event["headers"]["req_id"].as_str();
                    if ack.is_some() && ack == heartbeat_id.as_deref() {
                        ensure!(event["errcode"] == 0, "WeCom heartbeat was rejected");
                        heartbeat_id = None;
                    }
                    if pending.as_ref().is_some_and(|(id, _)| Some(id.as_str()) == ack)
                        && let Some((_, delivery)) = pending.take()
                    {
                        let _ = delivery.result.send(if event["errcode"] == 0 { Ok(()) } else { Err(None) });
                    }
                }
            }
        }
    }

    pub(crate) async fn send(&self, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let encoded = chat.strip_prefix("wcb_").ok_or(Some(400))?;
        let chat = String::from_utf8(hex::decode(encoded).map_err(|_| Some(400))?)
            .map_err(|_| Some(400))?;
        if !valid_user(&chat) {
            return Err(Some(400));
        }
        let outbound = self.outbound.lock().await.clone().ok_or(None)?;
        let (result, response) = oneshot::channel();
        timeout(Duration::from_secs(20), async {
            outbound
                .send(Delivery {
                    chat,
                    text: text.into(),
                    result,
                })
                .await
                .map_err(|_| None)?;
            response.await.map_err(|_| None)?
        })
        .await
        .map_err(|_| None)?
    }
}

fn frame(command: &str, id: &str, body: Value) -> Message {
    let mut request = json!({"cmd":command,"headers":{"req_id":id}});
    if !body.is_null() {
        request["body"] = body;
    }
    Message::Text(request.to_string().into())
}

fn valid_user(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 120
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

fn incoming(bot: &str, event: &Value) -> Option<Incoming> {
    let body = &event["body"];
    if event["cmd"] != "aibot_msg_callback"
        || body["aibotid"] != bot
        || body["chattype"] != "single"
        || body["msgtype"] != "text"
    {
        return None;
    }
    let chat = body["from"]["userid"]
        .as_str()
        .filter(|id| valid_user(id))?;
    let id = body["msgid"].as_str()?;
    let text = body["text"]["content"].as_str()?;
    if id.is_empty() || id.len() > 256 || text.trim().is_empty() {
        return None;
    }
    Some(Incoming {
        attachments: Vec::new(),
        update: format!("wcb_{id}"),
        chat: format!("wcb_{}", hex::encode(chat)),
        text: text.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_callbacks_bind_to_the_configured_bot_and_user() {
        let mut event = json!({"cmd":"aibot_msg_callback","body":{"aibotid":"bot","chattype":"single",
            "from":{"userid":"001"},"msgid":"message","msgtype":"text","text":{"content":"hello"}}});
        assert_eq!(incoming("bot", &event).unwrap().chat, "wcb_303031");
        assert!(incoming("other", &event).is_none());
        event["body"]["chattype"] = json!("group");
        assert!(incoming("bot", &event).is_none());
    }
}
