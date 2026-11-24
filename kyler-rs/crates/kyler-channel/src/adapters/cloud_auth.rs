use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, header::HeaderMap};
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents};
use serde_json::Value;
use tokio::sync::Mutex;

#[derive(Default)]
pub(super) struct Keys(Mutex<Option<(Instant, Value)>>);

impl Keys {
    pub(super) async fn verify(
        &self,
        http: &Client,
        url: &str,
        headers: &HeaderMap,
        endorsement: Option<&str>,
    ) -> Result<Value> {
        let token = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .context("Missing callback bearer token")?;
        ensure!(token.len() <= 16_384, "Oversized callback token");
        let (signed, signature) = token.rsplit_once('.').context("Invalid callback token")?;
        let (header, claims) = signed.split_once('.').context("Invalid callback token")?;
        let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header)?)?;
        let kid = header["kid"]
            .as_str()
            .filter(|v| !v.is_empty())
            .context("Missing signing key ID")?;
        ensure!(
            header["alg"] == "RS256",
            "Invalid callback signing algorithm"
        );
        let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims)?)?;
        let now = chrono::Utc::now().timestamp();
        ensure!(
            claims["exp"].as_i64().is_some_and(|time| time >= now - 60)
                && (claims["nbf"].is_null()
                    || claims["nbf"].as_i64().is_some_and(|time| time <= now + 60)),
            "Invalid callback token lifetime"
        );
        let mut cache = self.0.lock().await;
        if cache.as_ref().is_none_or(|(time, keys)| {
            time.elapsed() >= Duration::from_secs(3600)
                || (time.elapsed() >= Duration::from_secs(30) && find_key(keys, kid).is_none())
        }) {
            let keys: Value = http
                .get(url)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            *cache = Some((Instant::now(), keys));
        }
        let key = cache
            .as_ref()
            .and_then(|(_, keys)| find_key(keys, kid))
            .context("Unknown callback signing key")?;
        ensure!(key["kty"] == "RSA", "Invalid signing key type");
        if let Some(endorsement) = endorsement {
            ensure!(
                key["endorsements"]
                    .as_array()
                    .is_some_and(|values| values.iter().any(|value| value == endorsement)),
                "Signing key cannot endorse this channel"
            );
        }
        let n = URL_SAFE_NO_PAD.decode(key["n"].as_str().context("Missing RSA modulus")?)?;
        let e = URL_SAFE_NO_PAD.decode(key["e"].as_str().context("Missing RSA exponent")?)?;
        RsaPublicKeyComponents { n: &n, e: &e }
            .verify(
                &RSA_PKCS1_2048_8192_SHA256,
                signed.as_bytes(),
                &URL_SAFE_NO_PAD.decode(signature)?,
            )
            .map_err(|_| anyhow::anyhow!("Invalid callback signature"))?;
        Ok(claims)
    }
}

fn find_key<'a>(keys: &'a Value, kid: &str) -> Option<&'a Value> {
    keys["keys"]
        .as_array()?
        .iter()
        .find(|key| key["kid"] == kid)
}

#[derive(Default)]
pub(super) struct AccessToken(Mutex<Option<(Instant, Duration, String)>>);

impl AccessToken {
    pub(super) async fn get(
        &self,
        http: &Client,
        url: &str,
        form: impl FnOnce() -> Result<String> + Send,
    ) -> Result<String, Option<u16>> {
        let mut cache = self.0.lock().await;
        if let Some((time, ttl, token)) = cache.as_ref() {
            if time.elapsed() < *ttl {
                return Ok(token.clone());
            }
        }
        let response = http
            .post(url)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form().map_err(|_| Some(400))?)
            .send()
            .await
            .map_err(|_| None)?;
        let value = checked(response)
            .await?
            .json::<Value>()
            .await
            .map_err(|_| None)?;
        let token = value["access_token"]
            .as_str()
            .filter(|t| !t.is_empty())
            .ok_or(None)?;
        let ttl = value["expires_in"]
            .as_u64()
            .or_else(|| value["expires_in"].as_str()?.parse().ok())
            .unwrap_or(300)
            .saturating_sub(60);
        *cache = Some((Instant::now(), Duration::from_secs(ttl), token.to_owned()));
        Ok(token.to_owned())
    }

    pub(super) async fn check(
        &self,
        response: reqwest::Response,
        token: &str,
    ) -> Result<reqwest::Response, Option<u16>> {
        if response.status().as_u16() == 401 {
            let mut cache = self.0.lock().await;
            if cache.as_ref().is_some_and(|(_, _, saved)| saved == token) {
                *cache = None;
            }
        }
        checked(response).await
    }
}

async fn checked(response: reqwest::Response) -> Result<reqwest::Response, Option<u16>> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(Some(response.status().as_u16()))
    }
}

pub(super) fn opaque_id(part: &str) -> bool {
    (1..=256).contains(&part.len())
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub(super) fn digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
