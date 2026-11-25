mod socket;

use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use reqwest::header::HeaderMap;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{Incoming, ReplyReceipt, Settings, Webhook, adapter::client};

pub struct QqBot {
    pub(crate) app: String,
    pub(crate) websocket: bool,
    session: Mutex<socket::Session>,
    secret: String,
    signing: Ed25519KeyPair,
    http: reqwest::Client,
    access: Mutex<Option<(String, Instant)>>,
}

impl QqBot {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let app = settings.required("KYLER_QQBOT_APP_ID")?;
        let secret = settings.required("KYLER_QQBOT_APP_SECRET")?;
        ensure!(
            app.len() <= 32 && app.bytes().all(|byte| byte.is_ascii_digit()),
            "Invalid QQ Bot App ID"
        );
        Ok(Self {
            websocket: websocket_mode(settings)?,
            session: Mutex::new(Default::default()),
            app,
            signing: key(&secret)?,
            secret,
            http: client()?,
            access: Mutex::new(None),
        })
    }

    pub(crate) fn webhook(&self, headers: &HeaderMap, body: &[u8]) -> Result<Webhook> {
        ensure!(
            !self.websocket,
            "QQ Bot is configured for WebSocket delivery"
        );
        decode(&self.signing, headers, body, chrono::Utc::now().timestamp())
    }

    async fn token(&self) -> Result<String, Option<u16>> {
        let mut cache = self.access.lock().await;
        if let Some((token, until)) = &*cache
            && *until > Instant::now()
        {
            return Ok(token.clone());
        }
        let response = self
            .http
            .post("https://api.bot.qq.com/app/getAppAccessToken")
            .json(&json!({"appId":self.app,"clientSecret":self.secret}))
            .send()
            .await
            .map_err(|_| None)?;
        if !response.status().is_success() {
            return Err(Some(response.status().as_u16()));
        }
        let data: Value = response.json().await.map_err(|_| None)?;
        let access = data["access_token"].as_str().ok_or(None)?.to_owned();
        let expires = data["expires_in"]
            .as_u64()
            .or_else(|| data["expires_in"].as_str()?.parse().ok())
            .unwrap_or_default()
            .min(7200)
            .saturating_sub(60);
        *cache = Some((
            access.clone(),
            Instant::now() + Duration::from_secs(expires),
        ));
        Ok(access)
    }

    pub(crate) async fn send(
        &self,
        chat: &str,
        text: &str,
        receipt: Option<&ReplyReceipt>,
    ) -> Result<(), Option<u16>> {
        let openid = chat
            .strip_prefix("qq_")
            .filter(|openid| valid_openid(openid))
            .ok_or(Some(400))?;
        let receipt = receipt
            .filter(|receipt| receipt.expires > chrono::Utc::now().timestamp())
            .ok_or(Some(403))?;
        let message_id = receipt.message_id.strip_prefix("qq_").ok_or(Some(400))?;
        let token = self.token().await?;
        let response = self.http.post(format!("https://api.bot.qq.com/v2/users/{openid}/messages"))
            .header("authorization",format!("QQBot {token}"))
            .json(&json!({"content":text,"msg_type":0,"msg_id":message_id,"msg_seq":receipt.sequence}))
            .send().await.map_err(|_| None)?;
        let status = response.status();
        if status.as_u16() == 401 {
            *self.access.lock().await = None;
        }
        if status.is_success() {
            Ok(())
        } else {
            Err(Some(status.as_u16()))
        }
    }
}

fn websocket_mode(settings: &Settings<'_>) -> Result<bool> {
    let mode = settings
        .get("KYLER_QQBOT_MODE")
        .unwrap_or_else(|| "webhook".into());
    ensure!(
        matches!(mode.as_str(), "websocket" | "webhook"),
        "QQ Bot mode must be websocket or webhook"
    );
    Ok(mode == "websocket")
}

fn key(secret: &str) -> Result<Ed25519KeyPair> {
    ensure!(!secret.is_empty(), "Missing QQ Bot secret");
    let mut seed = [0; 32];
    for (target, source) in seed.iter_mut().zip(secret.bytes().cycle()) {
        *target = source;
    }
    Ed25519KeyPair::from_seed_unchecked(&seed)
        .map_err(|_| anyhow::anyhow!("Invalid QQ Bot signing key"))
}

fn valid_openid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn decode(key: &Ed25519KeyPair, headers: &HeaderMap, body: &[u8], now: i64) -> Result<Webhook> {
    let timestamp = headers
        .get("x-signature-timestamp")
        .context("Missing QQ timestamp")?
        .to_str()?;
    ensure!(
        timestamp.parse::<i64>()?.abs_diff(now) <= 300,
        "Expired QQ signature timestamp"
    );
    let mut signature = [0; 64];
    hex::decode_to_slice(
        headers
            .get("x-signature-ed25519")
            .context("Missing QQ signature")?
            .as_bytes(),
        &mut signature,
    )?;
    UnparsedPublicKey::new(&ED25519, key.public_key())
        .verify(&[timestamp.as_bytes(), body].concat(), &signature)
        .map_err(|_| anyhow::anyhow!("Invalid QQ signature"))?;
    let payload: Value = serde_json::from_slice(body)?;
    let data = &payload["d"];
    match payload["op"].as_u64() {
        Some(13) => {
            let token = data["plain_token"]
                .as_str()
                .context("Missing QQ validation token")?;
            let event_ts = data["event_ts"]
                .as_str()
                .context("Missing QQ validation timestamp")?;
            let signature = key.sign(&[event_ts.as_bytes(), token.as_bytes()].concat());
            Ok(Webhook::Response(
                json!({"plain_token":token,"signature":hex::encode(signature)}),
                Vec::new(),
            ))
        }
        Some(1) => Ok(Webhook::Response(json!({"op":11,"d":data}), Vec::new())),
        Some(0) => Ok(Webhook::Response(
            json!({"op":12,"d":0}),
            incoming(&payload, now).into_iter().collect(),
        )),
        _ => Ok(Webhook::Ignore),
    }
}

fn incoming(payload: &Value, now: i64) -> Option<Incoming> {
    if payload["t"] != "C2C_MESSAGE_CREATE" {
        return None;
    }
    let data = &payload["d"];
    let event_time = chrono::DateTime::parse_from_rfc3339(data["timestamp"].as_str()?)
        .ok()?
        .timestamp();
    let openid = data["author"]["user_openid"].as_str()?;
    let update = data["id"].as_str()?;
    let text = data["content"].as_str()?;
    if !valid_openid(openid)
        || update.is_empty()
        || update.len() > 256
        || text.trim().is_empty()
        || !(now - 3600..=now + 300).contains(&event_time)
    {
        return None;
    }
    Some(Incoming {
        update: format!("qq_{update}"),
        chat: format!("qq_{openid}"),
        text: text.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_mode_preserves_legacy_webhook_and_validates_explicit_selection() {
        let mut values = std::collections::BTreeMap::new();
        assert!(!websocket_mode(&Settings(&values)).unwrap());
        values.insert("KYLER_QQBOT_MODE".into(), "websocket".into());
        assert!(websocket_mode(&Settings(&values)).unwrap());
        values.insert("KYLER_QQBOT_MODE".into(), "webhook".into());
        assert!(!websocket_mode(&Settings(&values)).unwrap());
        values.insert("KYLER_QQBOT_MODE".into(), "typo".into());
        assert!(websocket_mode(&Settings(&values)).is_err());
    }

    #[test]
    fn signed_private_message_is_bound_to_its_reply_id() -> Result<()> {
        let key = key("test-secret")?;
        let body = serde_json::to_vec(&json!({"op":0,"t":"C2C_MESSAGE_CREATE","d":{
            "author":{"user_openid":"openid"},"id":"id-1","content":"hello","timestamp":"1970-01-01T00:16:40Z"}}))?;
        let mut headers = HeaderMap::new();
        headers.insert("x-signature-timestamp", "1000".parse()?);
        headers.insert(
            "x-signature-ed25519",
            hex::encode(key.sign(&[b"1000", &body[..]].concat())).parse()?,
        );
        assert!(
            matches!(decode(&key,&headers,&body,1000)?, Webhook::Response(response, messages)
            if response["op"] == 12 && messages.len() == 1 && messages[0].update == "qq_id-1")
        );
        assert!(decode(&key, &headers, b"{}", 1000).is_err());
        assert!(decode(&key, &headers, &body, 1301).is_err());
        Ok(())
    }
}
