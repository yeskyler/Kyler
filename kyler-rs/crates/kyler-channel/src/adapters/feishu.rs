use std::time::{Duration, Instant};

use anyhow::{Result, ensure};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

use crate::{Incoming, Settings, Webhook, adapter::client};

pub struct Feishu {
    http: reqwest::Client,
    pub(crate) app: String,
    pub(crate) tenant: String,
    secret: String,
    verification: String,
    token: Mutex<Option<(String, Instant)>>,
}

impl Feishu {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        Ok(Self {
            http: client()?,
            app: settings.required("KYLER_FEISHU_APP_ID")?,
            tenant: settings.required("KYLER_FEISHU_TENANT_KEY")?,
            secret: settings.required("KYLER_FEISHU_APP_SECRET")?,
            verification: settings.required("KYLER_FEISHU_VERIFICATION_TOKEN")?,
            token: Mutex::new(None),
        })
    }

    pub(crate) fn webhook(&self, body: &[u8]) -> Result<Webhook> {
        Self::parse(&self.verification, &self.app, &self.tenant, body)
    }

    fn parse(secret: &str, app: &str, tenant: &str, body: &[u8]) -> Result<Webhook> {
        let payload: Value = serde_json::from_slice(body)?;
        ensure!(
            payload.get("encrypt").is_none(),
            "Encrypted Feishu events are unsupported"
        );
        let header = &payload["header"];
        let challenge = payload["type"] == "url_verification";
        let verification = if challenge {
            &payload["token"]
        } else {
            &header["token"]
        };
        ensure!(
            verification
                .as_str()
                .is_some_and(|token| bool::from(token.as_bytes().ct_eq(secret.as_bytes()))),
            "Invalid Feishu verification token"
        );
        if challenge {
            let challenge = payload["challenge"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing Feishu challenge"))?;
            return Ok(Webhook::Challenge(challenge.into()));
        }
        ensure!(
            header["app_id"] == app && header["tenant_key"] == tenant,
            "Unexpected Feishu installation"
        );
        ensure!(header["create_time"].as_str().and_then(|time| time.parse::<i64>().ok())
            .is_some_and(|time| time.abs_diff(chrono::Utc::now().timestamp_millis()) <= 300_000),
            "Invalid Feishu event timestamp");
        let event = &payload["event"];
        let message = &event["message"];
        if payload["schema"] != "2.0"
            || header["event_type"] != "im.message.receive_v1"
            || event["sender"]["sender_type"] != "user"
            || message["chat_type"] != "p2p"
            || message["message_type"] != "text"
            || !message["thread_id"].is_null()
        {
            return Ok(Webhook::Ignore);
        }
        let content: Value = serde_json::from_str(message["content"].as_str().unwrap_or("{}"))?;
        match (
            header["event_id"].as_str(),
            event["sender"]["sender_id"]["open_id"].as_str(),
            content["text"].as_str(),
        ) {
            (Some(update), Some(chat), Some(text))
                if chat.starts_with("ou_")
                    && chat
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                    && !text.trim().is_empty() =>
            {
                Ok(Webhook::Messages(vec![Incoming {
                    update: update.into(),
                    chat: chat.into(),
                    text: text.into(),
                }]))
            }
            _ => Ok(Webhook::Ignore),
        }
    }

    async fn token(&self) -> Result<String, Option<u16>> {
        let mut cache = self.token.lock().await;
        if let Some((token, expiry)) = &*cache
            && *expiry > Instant::now()
        {
            return Ok(token.clone());
        }
        let response = self
            .http
            .post("https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal")
            .json(&json!({"app_id":self.app,"app_secret":self.secret}))
            .send()
            .await
            .map_err(|_| None)?;
        let body: Value = response.json().await.map_err(|_| None)?;
        if body["code"] != 0 {
            return Err(None);
        }
        let token = body["tenant_access_token"].as_str().ok_or(None)?.to_owned();
        let seconds = body["expire"]
            .as_u64()
            .unwrap_or(0)
            .min(7200)
            .saturating_sub(60);
        *cache = Some((token.clone(), Instant::now() + Duration::from_secs(seconds)));
        Ok(token)
    }

    pub(crate) async fn send(&self, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let credential = self.token().await?;
        let response = self.http.post("https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type=open_id")
            .bearer_auth(&credential)
            .json(&json!({"receive_id":chat,"msg_type":"text","content":json!({"text":text}).to_string()}))
            .send().await.map_err(|_| None)?;
        let status = response.status();
        if status.as_u16() == 401 {
            *self.token.lock().await = None;
        }
        if !status.is_success() {
            return Err(Some(status.as_u16()));
        }
        let body: Value = response.json().await.map_err(|_| None)?;
        if body["code"] == 0 { Ok(()) } else { Err(None) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feishu_requires_matching_token_and_tenant() -> Result<()> {
        for (tenant, token, allowed) in [
            ("tenant", "verify", true),
            ("other", "verify", false),
            ("tenant", "wrong", false),
        ] {
            let body = json!({"schema":"2.0","header":{"app_id":"cli_1","tenant_key":tenant,"token":token,
                "create_time":chrono::Utc::now().timestamp_millis().to_string(),"event_type":"im.message.receive_v1","event_id":"e1"},
                "event":{"sender":{"sender_type":"user","sender_id":{"open_id":"ou_1"}},"message":{"chat_type":"p2p","chat_id":"oc_1","message_type":"text","content":"{\"text\":\"hello\"}"}}});
            let outcome = Feishu::parse("verify", "cli_1", "tenant", &serde_json::to_vec(&body)?);
            assert_eq!(matches!(outcome, Ok(Webhook::Messages(_))), allowed);
            assert_eq!(outcome.is_ok(), allowed);
        }
        Ok(())
    }
}
