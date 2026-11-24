use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::header::HeaderMap;
use serde_json::{Value, json};

use crate::{
    Incoming, Settings, Webhook,
    adapter::{client, fresh, verify_hmac},
};

pub struct Line {
    http: reqwest::Client,
    access: String,
    secret: String,
    pub(crate) destination: String,
}

impl Line {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let destination = settings.required("KYLER_LINE_BOT_USER_ID")?;
        ensure!(user_id(&destination), "Invalid LINE bot user ID");
        Ok(Self {
            http: client()?,
            access: settings.required("KYLER_LINE_ACCESS_TOKEN")?,
            secret: settings.required("KYLER_LINE_CHANNEL_SECRET")?,
            destination,
        })
    }

    pub(crate) fn webhook(&self, headers: &HeaderMap, body: &[u8]) -> Result<Webhook> {
        Self::parse(&self.secret, &self.destination, headers, body)
    }

    fn parse(secret: &str, destination: &str, headers: &HeaderMap, body: &[u8]) -> Result<Webhook> {
        let signature = headers
            .get("x-line-signature")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        verify_hmac(secret, body, &STANDARD.decode(signature)?)?;
        let envelope: Value = serde_json::from_slice(body)?;
        ensure!(
            envelope["destination"] == destination,
            "Unexpected LINE destination"
        );
        let events = envelope["events"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Missing LINE events"))?;
        let messages = events
            .iter()
            .filter_map(|event| {
                let message = &event["message"];
                if event["type"] != "message"
                    || event["mode"] != "active"
                    || event["source"]["type"] != "user"
                    || message["type"] != "text"
                    || !event["timestamp"]
                        .as_i64()
                        .is_some_and(|timestamp| fresh(timestamp / 1000))
                {
                    return None;
                }
                let chat = event["source"]["userId"].as_str()?;
                let update = event["webhookEventId"].as_str()?;
                let text = message["text"].as_str()?;
                (user_id(chat) && !update.is_empty() && !text.trim().is_empty()).then(|| Incoming {
                    update: update.into(),
                    chat: chat.into(),
                    text: text.into(),
                })
            })
            .collect();
        Ok(Webhook::Messages(messages))
    }

    pub(crate) async fn send(&self, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let response = self
            .http
            .post("https://api.line.me/v2/bot/message/push")
            .bearer_auth(&self.access)
            .json(&json!({"to":chat,"messages":[{"type":"text","text":text}]}))
            .send()
            .await
            .map_err(|_| None)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(Some(status.as_u16()))
        }
    }
}

fn user_id(value: &str) -> bool {
    value.len() == 33
        && value.starts_with('U')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::{Hmac, KeyInit, Mac};
    use sha2_new::Sha256;

    #[test]
    fn validates_destination_signature_and_filters_non_private_events() -> Result<()> {
        let user = "U00000000000000000000000000000000";
        let event = |kind| {
            json!({"type":"message","mode":"active","source":{"type":kind,"userId":user},
            "webhookEventId":"event1","timestamp":chrono::Utc::now().timestamp_millis(),
            "message":{"type":"text","text":"hello"}})
        };
        let body = serde_json::to_vec(
            &json!({"destination":user,"events":[event("user"),event("group")]}),
        )?;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"test-secret")?;
        mac.update(&body);
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-line-signature",
            STANDARD.encode(mac.finalize().into_bytes()).parse()?,
        );
        assert!(
            matches!(Line::parse("test-secret", user, &headers, &body)?, Webhook::Messages(messages) if messages.len() == 1 && messages[0].chat == user)
        );
        assert!(Line::parse("test-secret", "other", &headers, &body).is_err());
        assert!(Line::parse("test-secret", user, &headers, b"{}").is_err());
        Ok(())
    }
}
