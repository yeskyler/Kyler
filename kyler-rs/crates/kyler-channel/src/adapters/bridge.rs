//! Kyler's signed transport for external channel bridges and API clients.
use anyhow::{Context, Result, ensure};
use hmac::{Hmac, KeyInit, Mac};
use reqwest::{Url, header::HeaderMap};
use serde::Deserialize;
use sha2_new::Sha256;
use subtle::ConstantTimeEq;

use crate::{Incoming, Settings, Webhook, adapter::client};

pub struct Bridge {
    pub(crate) profile: &'static str,
    pub(crate) instance: String,
    secret: String,
    destination: Option<(reqwest::Client, Url, String)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Event {
    id: String,
    chat: String,
    text: String,
    // The bridge asserts that this is a verified one-to-one conversation.
    private: bool,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        // SQLite's legacy columns have integer affinity. Require opaque, prefixed IDs
        // so large provider identifiers and leading zeros remain lossless.
        && id.bytes().any(|b| b.is_ascii_alphabetic())
}

impl Bridge {
    pub(crate) fn from_settings(profile: &'static str, settings: &Settings<'_>) -> Result<Self> {
        let prefix = format!("KYLER_CHANNEL_{}", profile.to_ascii_uppercase());
        let key = format!("{prefix}_SECRET");
        let secret = settings.required(&key)?;
        ensure!(secret.len() >= 32, "{key} must be at least 32 bytes");
        let instance = settings.required(&format!("{prefix}_INSTANCE"))?;
        ensure!(valid_id(&instance), "Invalid bridge instance ID");
        let destination = if profile == "api_server" {
            None
        } else {
            let raw = settings.required(&format!("{prefix}_REPLY_URL"))?;
            let url = Url::parse(&raw).map_err(|_| anyhow::anyhow!("Invalid bridge reply URL"))?;
            ensure!(
                url.scheme() == "https"
                    && url.host().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.fragment().is_none()
                    && url.query().is_none(),
                "Bridge reply URL must be HTTPS without embedded credentials, query or fragment"
            );
            let reply_secret = settings.required(&format!("{prefix}_REPLY_SECRET"))?;
            ensure!(
                reply_secret.len() >= 32 && reply_secret != secret,
                "Bridge reply secret must differ from the ingestion secret and be at least 32 bytes"
            );
            Some((client()?, url, reply_secret))
        };
        Ok(Self {
            profile,
            instance,
            secret,
            destination,
        })
    }

    pub(crate) fn webhook(&self, headers: &HeaderMap, body: &[u8]) -> Result<Webhook> {
        parse(
            &self.secret,
            self.profile,
            headers,
            body,
            chrono::Utc::now().timestamp(),
        )
    }

    pub fn pull_allowed(&self, credential: &str) -> bool {
        self.profile == "api_server"
            && bool::from(self.secret.as_bytes().ct_eq(credential.as_bytes()))
    }

    pub(crate) async fn send(&self, id: i64, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let (client, endpoint, secret) = self.destination.as_ref().ok_or(None)?;
        let body = serde_json::to_vec(&serde_json::json!({"channel":self.profile,"id":id.to_string(),"chat":chat,"text":text})).map_err(|_| None)?;
        let timestamp = chrono::Utc::now().timestamp().to_string();
        let mac = signed(secret, self.profile, "reply", &timestamp, &body).map_err(|_| None)?;
        let signature = hex::encode(mac.finalize().into_bytes());
        let response = client
            .post(endpoint.clone())
            .header("content-type", "application/json")
            .header("x-kyler-timestamp", timestamp)
            .header("x-kyler-signature", format!("sha256={signature}"))
            .body(body)
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

fn signed(
    secret: &str,
    profile: &str,
    direction: &str,
    timestamp: &str,
    body: &[u8],
) -> Result<Hmac<Sha256>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())?;
    mac.update(format!("v1:{direction}:{profile}:{timestamp}:").as_bytes());
    mac.update(body);
    Ok(mac)
}

fn parse(
    secret: &str,
    profile: &str,
    headers: &HeaderMap,
    body: &[u8],
    now: i64,
) -> Result<Webhook> {
    let timestamp = headers
        .get("x-kyler-timestamp")
        .context("Missing bridge timestamp")?
        .to_str()?;
    ensure!(
        timestamp.parse::<i64>()?.abs_diff(now) <= 300,
        "Expired bridge timestamp"
    );
    let signature = headers
        .get("x-kyler-signature")
        .context("Missing bridge signature")?
        .to_str()?;
    let digest = hex::decode(
        signature
            .strip_prefix("sha256=")
            .context("Invalid bridge signature")?,
    )?;
    signed(secret, profile, "event", timestamp, body)?
        .verify_slice(&digest)
        .map_err(|_| anyhow::anyhow!("Invalid bridge signature"))?;
    let event: Event = serde_json::from_slice(body)?;
    ensure!(
        valid_id(&event.id) && valid_id(&event.chat),
        "Invalid bridge identifier"
    );
    if !event.private || event.text.trim().is_empty() {
        return Ok(Webhook::Ignore);
    }
    Ok(Webhook::Messages(vec![Incoming {
        update: event.id,
        chat: event.chat,
        text: event.text,
    }]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_signatures_bind_body_direction_channel_and_time() -> Result<()> {
        let body = br#"{"id":"event_1","chat":"chat_1","text":"hello","private":true}"#;
        let mut headers = HeaderMap::new();
        headers.insert("x-kyler-timestamp", "1000".parse()?);
        for direction in ["event", "reply"] {
            let mac = signed("test", "discord", direction, "1000", body)?;
            let signature = hex::encode(mac.finalize().into_bytes());
            headers.insert("x-kyler-signature", format!("sha256={signature}").parse()?);
            assert_eq!(
                parse("test", "discord", &headers, body, 1000).is_ok(),
                direction == "event"
            );
            assert!(parse("test", "signal", &headers, body, 1000).is_err());
            assert!(parse("test", "discord", &headers, body, 1301).is_err());
            assert!(parse("test", "discord", &headers, b"{}", 1000).is_err());
        }
        Ok(())
    }
}
