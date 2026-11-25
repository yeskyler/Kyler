use std::time::{Duration, Instant};

use aes::Aes256;
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::NoPadding};
use serde::Deserialize;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

use crate::{Incoming, Settings, Webhook, adapter::client};

pub struct WeComCallback {
    http: reqwest::Client,
    pub(crate) corp: String,
    pub(crate) agent: u32,
    app_secret: String,
    verification_token: String,
    key: [u8; 32],
    access: Mutex<Option<(String, Instant)>>,
}

#[derive(Deserialize)]
struct Encrypted {
    #[serde(rename = "Encrypt")]
    body: String,
}

#[derive(Deserialize)]
struct Request {
    msg_signature: String,
    timestamp: String,
    nonce: String,
    echostr: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    #[serde(rename = "ToUserName")]
    corp: String,
    #[serde(rename = "FromUserName")]
    sender: String,
    #[serde(rename = "AgentID")]
    agent: Option<u32>,
    #[serde(rename = "MsgType")]
    kind: String,
    #[serde(rename = "Content")]
    content: Option<String>,
    #[serde(rename = "MsgId")]
    id: Option<String>,
}

impl WeComCallback {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let key = settings.required("KYLER_WECOM_CALLBACK_AES_KEY")?;
        ensure!(key.len() == 43, "Invalid WeCom callback AES key");
        let key = STANDARD
            .decode(format!("{key}="))
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .context("Invalid WeCom callback AES key")?;
        Ok(Self {
            http: client()?,
            corp: settings.required("KYLER_WECOM_CALLBACK_CORP_ID")?,
            agent: settings
                .required("KYLER_WECOM_CALLBACK_AGENT_ID")?
                .parse()
                .context("Invalid WeCom agent ID")?,
            app_secret: settings.required("KYLER_WECOM_CALLBACK_APP_SECRET")?,
            verification_token: settings.required("KYLER_WECOM_CALLBACK_TOKEN")?,
            key,
            access: Mutex::new(None),
        })
    }

    fn decode(&self, query: &str, ciphertext: Option<&str>) -> Result<String> {
        let query: Request = serde_urlencoded::from_str(query)?;
        let ciphertext = ciphertext
            .or(query.echostr.as_deref())
            .context("Missing WeCom ciphertext")?;
        ensure!(
            query
                .timestamp
                .parse::<i64>()?
                .abs_diff(chrono::Utc::now().timestamp())
                <= 300,
            "Expired WeCom callback timestamp"
        );
        let mut components = [
            self.verification_token.as_str(),
            &query.timestamp,
            &query.nonce,
            ciphertext,
        ];
        components.sort_unstable();
        let mut digest = Sha1::new();
        for piece in components {
            digest.update(piece.as_bytes());
        }
        let actual = hex::decode(query.msg_signature)?;
        ensure!(
            bool::from(digest.finalize().as_slice().ct_eq(&actual)),
            "Invalid WeCom signature"
        );
        decrypt(&self.key, &self.corp, ciphertext)
    }

    pub fn verify(&self, query: &str) -> Result<String> {
        self.decode(query, None)
    }

    pub fn webhook(&self, query: &str, body: &[u8]) -> Result<Webhook> {
        let encrypted: Encrypted = quick_xml::de::from_reader(body)?;
        let decrypted = self.decode(query, Some(&encrypted.body))?;
        // Event callbacks share the endpoint, but only text enters the pipeline.
        let message: Message = quick_xml::de::from_str(&decrypted)?;
        let incoming = match (message.id, message.content) {
            (Some(id), Some(text))
                if message.corp == self.corp
                    && message.agent == Some(self.agent)
                    && message.kind == "text"
                    && valid_sender(&message.sender)
                    && !id.is_empty()
                    && id.len() <= 64
                    && !text.trim().is_empty() =>
            {
                vec![Incoming {
                    attachments: Vec::new(),
                    update: format!("wc_{id}"),
                    chat: format!("wc_{}", message.sender),
                    text,
                }]
            }
            _ => Vec::new(),
        };
        Ok(Webhook::Plain("success", incoming))
    }

    async fn access_token(&self) -> Result<String, Option<u16>> {
        let mut cache = self.access.lock().await;
        if let Some((token, until)) = &*cache
            && *until > Instant::now()
        {
            return Ok(token.clone());
        }
        let response = self
            .http
            .get("https://qyapi.weixin.qq.com/cgi-bin/gettoken")
            .query(&[("corpid", &self.corp), ("corpsecret", &self.app_secret)])
            .send()
            .await
            .map_err(|_| None)?;
        let body: Value = response.json().await.map_err(|_| None)?;
        let token = body["access_token"]
            .as_str()
            .filter(|_| body["errcode"] == 0)
            .ok_or(None)?
            .to_owned();
        let expiry = body["expires_in"]
            .as_u64()
            .unwrap_or_default()
            .min(7200)
            .saturating_sub(60);
        *cache = Some((token.clone(), Instant::now() + Duration::from_secs(expiry)));
        Ok(token)
    }

    pub(crate) async fn send(&self, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let recipient = chat
            .strip_prefix("wc_")
            .filter(|id| valid_sender(id))
            .ok_or(Some(400))?;
        let token = self.access_token().await?;
        let response = self.http.post("https://qyapi.weixin.qq.com/cgi-bin/message/send")
            .query(&[("access_token", token)])
            .json(&json!({"touser":recipient,"agentid":self.agent,"msgtype":"text","text":{"content":text}}))
            .send().await.map_err(|_| None)?;
        let status = response.status();
        if !status.is_success() {
            return Err(Some(status.as_u16()));
        }
        let result: Value = response.json().await.map_err(|_| None)?;
        match result["errcode"].as_i64() {
            Some(0)
                if !result["invaliduser"]
                    .as_str()
                    .is_some_and(|ids| !ids.is_empty()) =>
            {
                Ok(())
            }
            Some(0 | 81013) => Err(Some(403)),
            Some(40014 | 42001) => {
                *self.access.lock().await = None;
                Err(None)
            }
            _ => Err(None),
        }
    }
}

fn valid_sender(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 120
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' || byte == b'.'
        })
        && value != "all"
}

fn decrypt(key: &[u8; 32], corp: &str, ciphertext: &str) -> Result<String> {
    let mut data = STANDARD.decode(ciphertext)?;
    let decoder = cbc::Decryptor::<Aes256>::new_from_slices(key, &key[..16])
        .map_err(|_| anyhow::anyhow!("Invalid WeCom cipher key"))?;
    let plain = decoder
        .decrypt_padded_mut::<NoPadding>(&mut data)
        .map_err(|_| anyhow::anyhow!("Invalid WeCom cipher payload"))?;
    let padding = usize::from(*plain.last().context("Empty WeCom ciphertext")?);
    ensure!(
        (1..=32).contains(&padding) && padding <= plain.len(),
        "Invalid WeCom cipher padding"
    );
    let (body, pad) = plain.split_at(plain.len() - padding);
    ensure!(
        pad.iter().all(|byte| usize::from(*byte) == padding),
        "Invalid WeCom cipher padding"
    );
    let size = u32::from_be_bytes(
        body.get(16..20)
            .context("Invalid WeCom cipher header")?
            .try_into()?,
    ) as usize;
    let body = &body[20..];
    ensure!(size <= body.len(), "Invalid WeCom message length");
    let (message, receiver) = body.split_at(size);
    ensure!(
        bool::from(receiver.ct_eq(corp.as_bytes())),
        "Unexpected WeCom receiver"
    );
    Ok(std::str::from_utf8(message)?.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cbc::cipher::BlockEncryptMut;

    #[test]
    fn checks_the_corporate_receiver_and_protocol_padding() -> Result<()> {
        let key = [7; 32];
        let mut bytes = vec![0; 16];
        bytes.extend_from_slice(&5u32.to_be_bytes());
        bytes.extend_from_slice(b"hellocorp");
        let pad = 32 - bytes.len() % 32;
        bytes.extend(vec![pad as u8; pad]);
        let count = bytes.len();
        let cipher = cbc::Encryptor::<Aes256>::new_from_slices(&key, &key[..16])
            .unwrap()
            .encrypt_padded_mut::<NoPadding>(&mut bytes, count)
            .unwrap();
        let encoded = STANDARD.encode(cipher);
        assert_eq!(decrypt(&key, "corp", &encoded)?, "hello");
        assert!(decrypt(&key, "other", &encoded).is_err());
        Ok(())
    }
}
