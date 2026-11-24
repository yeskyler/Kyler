use anyhow::{Context, Result, ensure};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use reqwest::{Url, header::HeaderMap};
use ring::{
    rand::SystemRandom,
    signature::{RSA_PKCS1_SHA256, RsaKeyPair},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::cloud_auth::{AccessToken, Keys, digest, opaque_id};
use crate::{Incoming, Settings, Webhook, adapter::client};

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

#[derive(Deserialize)]
struct Credentials {
    client_email: String,
    private_key_id: String,
    private_key: String,
}

pub struct GoogleChat {
    http: reqwest::Client,
    audience: String,
    email: String,
    key_id: String,
    key: RsaKeyPair,
    keys: Keys,
    access: AccessToken,
}

impl GoogleChat {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let audience = settings.required("KYLER_GOOGLE_CHAT_WEBHOOK_URL")?;
        let url = Url::parse(&audience)
            .map_err(|_| anyhow::anyhow!("Invalid Google Chat webhook URL"))?;
        ensure!(
            url.scheme() == "https"
                && url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path().ends_with("/webhooks/channels/google_chat"),
            "Invalid Google Chat webhook URL"
        );
        let credentials: Credentials = serde_json::from_slice(
            &std::fs::read(settings.required("KYLER_GOOGLE_CHAT_CREDENTIALS_FILE")?)
                .context("Cannot read Google Chat credentials file")?,
        )
        .map_err(|_| anyhow::anyhow!("Invalid Google Chat credentials file"))?;
        let key: String = credentials
            .private_key
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        let key = RsaKeyPair::from_pkcs8(&STANDARD.decode(key)?)
            .map_err(|_| anyhow::anyhow!("Invalid Google Chat RSA private key"))?;
        Ok(Self {
            http: client()?,
            audience,
            email: credentials.client_email,
            key_id: credentials.private_key_id,
            key,
            keys: Keys::default(),
            access: AccessToken::default(),
        })
    }

    pub(crate) fn namespace(&self) -> String {
        format!("google_chat:{}:{}", self.email, self.audience)
    }

    pub(crate) async fn webhook(&self, headers: &HeaderMap, body: &[u8]) -> Result<Webhook> {
        let claims = self
            .keys
            .verify(
                &self.http,
                "https://www.googleapis.com/oauth2/v3/certs",
                headers,
                None,
            )
            .await?;
        ensure!(
            matches!(
                claims["iss"].as_str(),
                Some("https://accounts.google.com" | "accounts.google.com")
            ) && claims["aud"] == self.audience
                && claims["email"] == "chat@system.gserviceaccount.com"
                && claims["email_verified"] == true,
            "Invalid Google Chat callback identity"
        );
        Ok(Webhook::Messages(
            message(&serde_json::from_slice(body)?)
                .into_iter()
                .collect(),
        ))
    }

    fn assertion(&self) -> Result<String> {
        let now = chrono::Utc::now().timestamp();
        let header = URL_SAFE_NO_PAD
            .encode(json!({"alg":"RS256","typ":"JWT","kid":self.key_id}).to_string());
        let claims = URL_SAFE_NO_PAD.encode(
            json!({"iss":self.email,"aud":TOKEN_URL,
            "scope":"https://www.googleapis.com/auth/chat.bot","iat":now,"exp":now+3600})
            .to_string(),
        );
        let payload = format!("{header}.{claims}");
        let mut signature = vec![0; self.key.public().modulus_len()];
        self.key
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                payload.as_bytes(),
                &mut signature,
            )
            .map_err(|_| anyhow::anyhow!("Cannot sign Google Chat service assertion"))?;
        Ok(format!("{payload}.{}", URL_SAFE_NO_PAD.encode(signature)))
    }

    pub(crate) async fn send(&self, id: i64, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let space = chat
            .strip_prefix("gc_")
            .filter(|space| opaque_id(space))
            .ok_or(Some(400))?;
        let token = self
            .access
            .get(&self.http, TOKEN_URL, || {
                Ok(serde_urlencoded::to_string([
                    ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                    ("assertion", self.assertion()?.as_str()),
                ])?)
            })
            .await?;
        let mut url = Url::parse(&format!(
            "https://chat.googleapis.com/v1/spaces/{space}/messages"
        ))
        .map_err(|_| Some(400))?;
        url.query_pairs_mut()
            .append_pair("requestId", &format!("kyler-{id}"));
        let response = self
            .http
            .post(url)
            .bearer_auth(&token)
            .json(&json!({"text":text}))
            .send()
            .await
            .map_err(|_| None)?;
        self.access.check(response, &token).await?;
        Ok(())
    }
}

fn message(event: &Value) -> Option<Incoming> {
    if event["type"] != "MESSAGE"
        || event["user"]["type"] != "HUMAN"
        || !(event["space"]["spaceType"] == "DIRECT_MESSAGE" || event["space"]["type"] == "DM")
    {
        return None;
    }
    let name = event["space"]["name"].as_str()?;
    let space = name
        .strip_prefix("spaces/")
        .filter(|space| opaque_id(space))?;
    let full_id = event["message"]["name"].as_str()?;
    let part = full_id.strip_prefix(name)?.strip_prefix("/messages/")?;
    let text = event["message"]["text"].as_str()?;
    if part.is_empty() || part.contains('/') || text.trim().is_empty() {
        return None;
    }
    Some(Incoming {
        update: format!("gc_{}", digest(full_id)),
        chat: format!("gc_{space}"),
        text: text.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_human_direct_messages_in_the_named_space() {
        let mut event = json!({"type":"MESSAGE","user":{"type":"HUMAN"},
            "space":{"name":"spaces/abc","spaceType":"DIRECT_MESSAGE"},
            "message":{"name":"spaces/abc/messages/one","text":"hello"}});
        assert_eq!(message(&event).unwrap().chat, "gc_abc");
        event["message"]["name"] = "spaces/other/messages/one".into();
        assert!(message(&event).is_none());
    }
}
