use std::time::Duration;

use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{RequestBuilder, Url};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{ChannelStore, Incoming, ReplyReceipt, Settings, adapter::client};

const PROTOCOL_VERSION: &str = "2.4.8";
const CLIENT_VERSION: &str = "132104";

pub struct Weixin {
    http: reqwest::Client,
    base: Url,
    bot: String,
    user: String,
    token: String,
}

impl Weixin {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let raw = settings
            .get("KYLER_WEIXIN_URL")
            .unwrap_or_else(|| "https://ilinkai.weixin.qq.com/".into());
        let base = Url::parse(&raw).map_err(|_| anyhow::anyhow!("Invalid Weixin iLink URL"))?;
        ensure!(
            base.scheme() == "https"
                && base
                    .host_str()
                    .is_some_and(|host| host.ends_with(".weixin.qq.com"))
                && base.path() == "/"
                && base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none(),
            "Weixin URL must be an official HTTPS iLink origin"
        );
        let bot = settings.required("KYLER_WEIXIN_BOT_ID")?;
        let user = settings.required("KYLER_WEIXIN_USER_ID")?;
        ensure!(
            valid_id(&bot, "@im.bot") && valid_id(&user, "@im.wechat"),
            "Invalid Weixin bot or user ID"
        );
        Ok(Self {
            http: client()?,
            base,
            bot,
            user,
            token: settings.required("KYLER_WEIXIN_BOT_TOKEN")?,
        })
    }

    pub(crate) fn namespace(&self) -> String {
        format!("weixin:{}:{}", self.bot, self.user)
    }

    fn post(&self, method: &str, mut body: Value) -> Result<RequestBuilder> {
        let random = Uuid::new_v4();
        let random = u32::from_be_bytes(random.as_bytes()[..4].try_into()?);
        body["base_info"] = json!({"channel_version":PROTOCOL_VERSION,"bot_agent":"Kyler"});
        Ok(self
            .http
            .post(self.base.join(&format!("ilink/bot/{method}"))?)
            .header("authorizationtype", "ilink_bot_token")
            .bearer_auth(&self.token)
            .header("x-wechat-uin", STANDARD.encode(random.to_string()))
            .header("ilink-app-id", "bot")
            .header("ilink-app-clientversion", CLIENT_VERSION)
            .json(&body))
    }

    pub async fn listen(&self, store: &ChannelStore) -> Result<()> {
        let mut cursor = store.cursor().await?;
        let mut poll_timeout = Duration::from_secs(45);
        loop {
            let result: Value = self
                .post("getupdates", json!({"get_updates_buf":cursor}))?
                .timeout(poll_timeout)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            ensure!(success(&result), "Weixin rejected the poll");
            poll_timeout = Duration::from_millis(
                result["longpolling_timeout_ms"]
                    .as_u64()
                    .unwrap_or(35_000)
                    .clamp(2000, 55_000)
                    + 5000,
            );
            let messages: Vec<_> = result["msgs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|message| incoming(&self.user, &self.bot, message))
                .map(|(message, token)| (message, Some(token)))
                .collect();
            let next = result["get_updates_buf"]
                .as_str()
                .filter(|next| !next.is_empty() && *next != cursor)
                .map(str::to_owned);
            let idle = messages.is_empty() && next.is_none();
            store.ingest_batch(messages, next.clone()).await?;
            if let Some(next) = next {
                cursor = next;
            }
            if idle {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }

    pub(crate) async fn send(
        &self,
        id: i64,
        chat: &str,
        text: &str,
        receipt: Option<&ReplyReceipt>,
    ) -> Result<(), Option<u16>> {
        if chat.strip_prefix("wx_") != Some(hex::encode(&self.user).as_str()) {
            return Err(Some(403));
        }
        let context = receipt
            .and_then(|receipt| receipt.context_token.as_deref())
            .ok_or(Some(403))?;
        let response = self
            .post(
                "sendmessage",
                json!({"msg":{
                    "from_user_id":"","to_user_id":self.user,"client_id":format!("kyler-{id}"),
                    "message_type":2,"message_state":2,"context_token":context,
                    "item_list":[{"type":1,"text_item":{"text":text}}]
                }}),
            )
            .map_err(|_| None)?
            .send()
            .await
            .map_err(|_| None)?;
        if !response.status().is_success() {
            return Err(Some(response.status().as_u16()));
        }
        let result: Value = response.json().await.map_err(|_| None)?;
        if success(&result) {
            Ok(())
        } else {
            match ["ret", "errcode"]
                .iter()
                .filter_map(|key| result[key].as_i64())
                .find(|code| *code != 0)
            {
                Some(-2) => Err(Some(403)),
                _ => Err(None),
            }
        }
    }
}

fn success(result: &Value) -> bool {
    result.is_object()
        && ["ret", "errcode"]
            .iter()
            .all(|key| result.get(key).is_none_or(|value| value == 0))
}

fn valid_id(value: &str, suffix: &str) -> bool {
    value.strip_suffix(suffix).is_some_and(|name| {
        !name.is_empty()
            && name.len() <= 180
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    })
}

fn incoming(user: &str, bot: &str, value: &Value) -> Option<(Incoming, String)> {
    if value["from_user_id"] != user
        || value["message_type"] != 1
        || value["to_user_id"]
            .as_str()
            .is_some_and(|to| !to.is_empty() && to != bot)
        || value["group_id"]
            .as_str()
            .is_some_and(|group| !group.is_empty())
        || value["delete_time_ms"]
            .as_u64()
            .is_some_and(|deleted| deleted != 0)
    {
        return None;
    }
    let id = value["message_id"].as_u64().filter(|id| *id != 0)?;
    let token = value["context_token"].as_str()?;
    let text = value["item_list"]
        .as_array()?
        .iter()
        .filter(|item| item["type"] == 1)
        .filter_map(|item| item["text_item"]["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if token.is_empty() || token.len() > 4096 || text.trim().is_empty() {
        return None;
    }
    Some((
        Incoming {
            attachments: Vec::new(),
            update: format!("wx_{id}"),
            chat: format!("wx_{}", hex::encode(user)),
            text,
        },
        token.into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_text_requires_the_authorized_peer_and_reply_token() {
        let bot = "bot@im.bot";
        let user = "peer@im.wechat";
        let mut event = json!({"from_user_id":user,"to_user_id":bot,"message_type":1,"message_id":12,
            "context_token":"token","item_list":[{"type":1,"text_item":{"text":"hello"}}]});
        let (message, context) = incoming(user, bot, &event).unwrap();
        assert_eq!(message.update, "wx_12");
        assert_eq!(context, "token");
        assert!(incoming("other@im.wechat", bot, &event).is_none());
        event["group_id"] = json!("group");
        assert!(incoming(user, bot, &event).is_none());
        assert!(!success(&json!({"ret":0,"errcode":-14})));
    }
}
