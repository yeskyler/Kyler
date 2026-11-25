use std::time::Duration;

use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::Incoming;

pub struct Telegram {
    client: reqwest::Client,
    endpoint: String,
    pub bot_id: i64,
}

#[derive(Deserialize)]
struct Envelope<T> {
    ok: bool,
    result: Option<T>,
    error_code: Option<u16>,
}

#[derive(Deserialize)]
struct Update {
    update_id: i64,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    chat: Chat,
    from: Option<User>,
    text: Option<String>,
}

#[derive(Deserialize)]
struct Chat {
    id: i64,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct User {
    id: i64,
    is_bot: bool,
}

impl Telegram {
    pub fn new(token: &str) -> Result<Self> {
        let (id, secret) = token
            .split_once(':')
            .ok_or_else(|| anyhow!("Invalid Telegram bot token"))?;
        let bot_id: i64 = id
            .parse()
            .map_err(|_| anyhow!("Invalid Telegram bot token"))?;
        ensure!(
            bot_id > 0
                && !secret.is_empty()
                && secret
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "Invalid Telegram bot token"
        );
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(40))
                .build()?,
            endpoint: format!("https://api.telegram.org/bot{token}"),
            bot_id,
        })
    }

    async fn call<T: DeserializeOwned>(&self, method: &str, body: Value) -> Result<T, Option<u16>> {
        // HTTP errors can contain the request URL, which contains the bot secret.
        let response = self
            .client
            .post(format!("{}/{method}", self.endpoint))
            .json(&body)
            .send()
            .await
            .map_err(|_| None)?;
        let status = response.status();
        if !status.is_success() {
            return Err(Some(status.as_u16()));
        }
        let envelope: Envelope<T> = response.json().await.map_err(|_| None)?;
        if envelope.ok {
            envelope.result.ok_or(None)
        } else {
            Err(envelope.error_code)
        }
    }

    pub async fn poll(&self, offset: Option<i64>) -> Result<(Option<i64>, Vec<Incoming>)> {
        let mut body = json!({"timeout":25, "allowed_updates":["message"]});
        if let Some(offset) = offset {
            body["offset"] = json!(offset);
        }
        let mut updates: Vec<Update> = self
            .call("getUpdates", body)
            .await
            .map_err(|status| anyhow!("Telegram polling failed (HTTP status: {status:?})"))?;
        let next = updates
            .iter()
            .map(|update| update.update_id)
            .max()
            .and_then(|id| id.checked_add(1));
        updates.sort_unstable_by_key(|update| update.update_id);
        let messages = updates
            .into_iter()
            .filter_map(Update::into_incoming)
            .collect();
        Ok((next, messages))
    }

    /// Telegram may reject delivery permanently when a user blocks the bot.
    pub async fn send(&self, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let chat_id = chat.parse::<i64>().map_err(|_| Some(400))?;
        self.call::<Value>(
            "sendMessage",
            json!({
                "chat_id":chat_id, "text":text, "link_preview_options":{"is_disabled":true}
            }),
        )
        .await
        .map(|_| ())
    }
}

impl Update {
    fn into_incoming(self) -> Option<Incoming> {
        let message = self.message?;
        let user = message.from?;
        let text = message.text?;
        (message.chat.kind == "private"
            && !user.is_bot
            && user.id == message.chat.id
            && !text.trim().is_empty())
        .then_some(Incoming {
            attachments: Vec::new(),
            update: self.update_id.to_string(),
            chat: user.id.to_string(),
            text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_direct_user_text() {
        for (kind, from, bot, accepted) in [
            ("private", 10, false, true),
            ("private", 11, false, false),
            ("private", 10, true, false),
            ("group", 10, false, false),
        ] {
            let update: Update = serde_json::from_value(json!({"update_id":1,"message":{
                "chat":{"id":10,"type":kind},"from":{"id":from,"is_bot":bot},"text":"hello"
            }}))
            .unwrap();
            assert_eq!(update.into_incoming().is_some(), accepted);
        }
    }
}
