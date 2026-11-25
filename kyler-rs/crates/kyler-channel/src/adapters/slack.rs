use anyhow::{Result, ensure};
use hmac::{Hmac, KeyInit, Mac};
use reqwest::header::HeaderMap;
use serde_json::{Value, json};
use sha2_new::Sha256;
use subtle::ConstantTimeEq;

use crate::{Incoming, Settings, Webhook, adapter::client};

pub struct Slack {
    http: reqwest::Client,
    token: String,
    signing_secret: String,
    pub(crate) team: String,
    pub(crate) app: String,
}

fn signature_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut signature = String::with_capacity(3 + bytes.len() * 2);
    signature.push_str("v0=");
    for byte in bytes {
        signature.push(HEX[(byte >> 4) as usize] as char);
        signature.push(HEX[(byte & 15) as usize] as char);
    }
    signature
}

impl Slack {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        Ok(Self {
            http: client()?,
            token: settings.required("KYLER_SLACK_BOT_TOKEN")?,
            signing_secret: settings.required("KYLER_SLACK_SIGNING_SECRET")?,
            team: settings.required("KYLER_SLACK_TEAM_ID")?,
            app: settings.required("KYLER_SLACK_APP_ID")?,
        })
    }

    pub(crate) fn webhook(&self, headers: &HeaderMap, body: &[u8]) -> Result<Webhook> {
        Self::parse(&self.signing_secret, &self.team, &self.app, headers, body)
    }

    fn parse(
        secret: &str,
        team: &str,
        app: &str,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<Webhook> {
        let timestamp = headers
            .get("x-slack-request-timestamp")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let signature = headers
            .get("x-slack-signature")
            .map_or(&[][..], |value| value.as_bytes());
        ensure!(
            timestamp
                .parse::<i64>()
                .is_ok_and(|time| time.abs_diff(chrono::Utc::now().timestamp()) <= 300),
            "Invalid Slack timestamp"
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())?;
        mac.update(format!("v0:{timestamp}:").as_bytes());
        mac.update(body);
        let digest = mac.finalize().into_bytes();
        let expected = signature_hex(&digest);
        ensure!(
            bool::from(expected.as_bytes().ct_eq(signature)),
            "Invalid Slack signature"
        );
        let payload: Value = serde_json::from_slice(body)?;
        if payload["type"] == "url_verification" {
            let challenge = payload["challenge"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing Slack challenge"))?;
            return Ok(Webhook::Challenge(challenge.into()));
        }
        ensure!(
            payload["team_id"] == team && payload["api_app_id"] == app,
            "Unexpected Slack installation"
        );
        let event = &payload["event"];
        if payload["type"] != "event_callback"
            || event["type"] != "message"
            || !matches!(event["channel_type"].as_str(), Some("im" | "app_home"))
            || !event["subtype"].is_null()
            || !event["bot_id"].is_null()
            || !event["thread_ts"].is_null()
            || event["user"].as_str().is_none()
        {
            return Ok(Webhook::Ignore);
        }
        match (
            payload["event_id"].as_str(),
            event["channel"].as_str(),
            event["text"].as_str(),
        ) {
            (Some(update), Some(chat), Some(text))
                if chat.starts_with('D')
                    && chat.bytes().all(|b| b.is_ascii_alphanumeric())
                    && !text.trim().is_empty() =>
            {
                Ok(Webhook::Messages(vec![Incoming {
                    attachments: Vec::new(),
                    update: update.into(),
                    chat: chat.into(),
                    text: text.into(),
                }]))
            }
            _ => Ok(Webhook::Ignore),
        }
    }

    pub(crate) async fn send(&self, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let response = self.http.post("https://slack.com/api/chat.postMessage")
            .bearer_auth(&self.token)
            .json(&json!({"channel":chat,"text":text,"unfurl_links":false,"unfurl_media":false,"mrkdwn":false}))
            .send().await.map_err(|_| None)?;
        if !response.status().is_success() {
            return Err(Some(response.status().as_u16()));
        }
        let body: Value = response.json().await.map_err(|_| None)?;
        if body["ok"] == true {
            return Ok(());
        }
        match body["error"].as_str() {
            Some("channel_not_found" | "not_in_channel" | "is_archived") => Err(Some(403)),
            _ => Err(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slack_rejects_tampered_body_and_group_messages() -> Result<()> {
        let now = chrono::Utc::now().timestamp().to_string();
        let mut headers = HeaderMap::new();
        headers.insert("x-slack-request-timestamp", now.parse()?);
        for (kind, accepted) in [("im", true), ("group", false)] {
            let body = json!({"type":"event_callback","team_id":"T1","api_app_id":"A1","event_id":"Ev1",
                "event":{"type":"message","channel_type":kind,"channel":"D1","user":"U1","text":"hello"}}).to_string();
            let mut mac = Hmac::<Sha256>::new_from_slice(b"secret")?;
            mac.update(format!("v0:{now}:{body}").as_bytes());
            let digest = mac.finalize().into_bytes();
            headers.insert("x-slack-signature", signature_hex(&digest).parse()?);
            assert_eq!(
                matches!(
                    Slack::parse("secret", "T1", "A1", &headers, body.as_bytes())?,
                    Webhook::Messages(_)
                ),
                accepted
            );
            assert!(Slack::parse("secret", "T1", "A1", &headers, b"{}").is_err());
        }
        Ok(())
    }
}
