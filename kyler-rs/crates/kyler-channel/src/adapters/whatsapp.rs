use anyhow::{Result, ensure};
use reqwest::header::HeaderMap;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;

use crate::{
    Incoming, Settings, Webhook,
    adapter::{client, fresh, verify_hmac},
};

pub struct WhatsApp {
    http: reqwest::Client,
    access: String,
    app_secret: String,
    verify_token: String,
    endpoint: String,
    pub(crate) phone: String,
}

impl WhatsApp {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let phone = settings.required("KYLER_WHATSAPP_PHONE_NUMBER_ID")?;
        let version = settings.required("KYLER_WHATSAPP_GRAPH_VERSION")?;
        ensure!(
            digits(&phone)
                && version
                    .strip_prefix('v')
                    .and_then(|part| part.split_once('.'))
                    .is_some_and(|(major, minor)| digits(major) && digits(minor)),
            "Invalid WhatsApp phone number ID or Graph API version"
        );
        Ok(Self {
            http: client()?,
            access: settings.required("KYLER_WHATSAPP_ACCESS_TOKEN")?,
            app_secret: settings.required("KYLER_WHATSAPP_APP_SECRET")?,
            verify_token: settings.required("KYLER_WHATSAPP_VERIFY_TOKEN")?,
            endpoint: format!("https://graph.facebook.com/{version}/{phone}/messages"),
            phone,
        })
    }

    pub(crate) fn verify_subscription(&self, mode: &str, token: &str) -> bool {
        subscription_matches(&self.verify_token, mode, token)
    }

    pub(crate) fn webhook(&self, headers: &HeaderMap, body: &[u8]) -> Result<Webhook> {
        Self::parse(&self.app_secret, &self.phone, headers, body)
    }

    fn parse(secret: &str, phone: &str, headers: &HeaderMap, body: &[u8]) -> Result<Webhook> {
        let digest = headers
            .get("x-hub-signature-256")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("sha256="))
            .unwrap_or("");
        verify_hmac(secret, body, &hex::decode(digest)?)?;
        let envelope: Value = serde_json::from_slice(body)?;
        if envelope["object"] != "whatsapp_business_account" {
            return Ok(Webhook::Ignore);
        }
        let messages = items(&envelope["entry"])
            .flat_map(|entry| items(&entry["changes"]))
            .filter(|change| {
                change["field"] == "messages"
                    && change["value"]["metadata"]["phone_number_id"] == phone
            })
            .flat_map(|change| items(&change["value"]["messages"]))
            .filter_map(|message| {
                if message["type"] != "text"
                    || !message["group_id"].is_null()
                    || !message["timestamp"]
                        .as_str()
                        .and_then(|value| value.parse().ok())
                        .is_some_and(fresh)
                {
                    return None;
                }
                let chat = message["from"].as_str()?;
                let update = message["id"].as_str()?;
                let text = message["text"]["body"].as_str()?;
                (digits(chat) && !update.is_empty() && !text.trim().is_empty()).then(|| Incoming {
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
            .post(&self.endpoint)
            .bearer_auth(&self.access)
            .json(
                &json!({"messaging_product":"whatsapp","recipient_type":"individual","to":chat,
                "type":"text","text":{"preview_url":false,"body":text}}),
            )
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

fn subscription_matches(secret: &str, mode: &str, token: &str) -> bool {
    mode == "subscribe" && bool::from(secret.as_bytes().ct_eq(token.as_bytes()))
}

fn digits(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn items(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::{Hmac, KeyInit, Mac};
    use sha2_new::Sha256;

    #[test]
    fn verifies_subscription_and_filters_batched_events_to_configured_number() -> Result<()> {
        assert!(subscription_matches(
            "test-token",
            "subscribe",
            "test-token"
        ));
        assert!(!subscription_matches("test-token", "subscribe", "other"));
        assert!(!subscription_matches("test-token", "other", "test-token"));
        let change = |phone| {
            json!({"field":"messages","value":{"metadata":{"phone_number_id":phone},
            "messages":[{"from":"12345","id":"wamid.1","timestamp":chrono::Utc::now().timestamp().to_string(),
                "type":"text","text":{"body":"hello"}}]}})
        };
        let body = serde_json::to_vec(&json!({"object":"whatsapp_business_account","entry":[
            {"changes":[change("123"),change("456")]}
        ]}))?;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"test-secret")?;
        mac.update(&body);
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-hub-signature-256",
            format!("sha256={}", hex::encode(mac.finalize().into_bytes())).parse()?,
        );
        assert!(
            matches!(WhatsApp::parse("test-secret", "123", &headers, &body)?, Webhook::Messages(messages) if messages.len() == 1 && messages[0].update == "wamid.1")
        );
        assert!(WhatsApp::parse("test-secret", "123", &headers, b"{}").is_err());
        Ok(())
    }
}
