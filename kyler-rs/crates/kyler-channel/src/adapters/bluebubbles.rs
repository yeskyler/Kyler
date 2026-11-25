use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::Url;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{ChannelStore, Incoming, Settings, adapter::client};

pub struct BlueBubbles {
    http: reqwest::Client,
    base: Url,
    password: String,
    cursors: Mutex<BTreeMap<String, i64>>,
}

impl BlueBubbles {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let mut base = Url::parse(&settings.required("KYLER_BLUEBUBBLES_URL")?)
            .map_err(|_| anyhow::anyhow!("Invalid BlueBubbles URL"))?;
        ensure!(
            base.scheme() == "https"
                && base.host().is_some()
                && base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none(),
            "BlueBubbles URL must be HTTPS without credentials, query, or fragment"
        );
        if !base.path().ends_with('/') {
            base.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("Invalid BlueBubbles path"))?
                .push("");
        }
        let chats = settings.required("KYLER_BLUEBUBBLES_CHATS")?;
        let mut cursors = BTreeMap::new();
        let now = chrono::Utc::now().timestamp_millis();
        for chat in chats.split(',').map(str::trim) {
            ensure!(
                valid_chat(chat),
                "BlueBubbles requires direct iMessage chat GUIDs"
            );
            cursors.insert(chat.to_owned(), now);
        }
        Ok(Self {
            http: client()?,
            base,
            password: settings.required("KYLER_BLUEBUBBLES_PASSWORD")?,
            cursors: Mutex::new(cursors),
        })
    }

    pub(crate) fn namespace(&self) -> String {
        format!("bluebubbles:{}", self.base)
    }

    pub async fn listen(&self, store: &ChannelStore) -> Result<()> {
        let endpoint = self.base.join("api/v1/message/query")?;
        loop {
            // Never hold the cursor lock across network calls, since replies need it too.
            let chats = self.cursors.lock().await.clone();
            for (chat, after) in chats {
                let before = chrono::Utc::now().timestamp_millis();
                if after >= before {
                    continue;
                }
                let mut offset = 0;
                loop {
                    let response: Value = self
                        .http
                        .post(endpoint.clone())
                        .query(&[("password", &self.password)])
                        .json(&json!({"chatGuid":chat,"after":after,"before":before,
                            "limit":100,"offset":offset,"sort":"ASC"}))
                        .send()
                        .await?
                        .error_for_status()?
                        .json()
                        .await?;
                    let rows = response["data"]
                        .as_array()
                        .context("Invalid BlueBubbles query response")?;
                    let incoming = rows
                        .iter()
                        .filter_map(|row| message(&chat, after, before, row))
                        .collect();
                    store.ingest(incoming).await?;
                    if rows.len() < 100 {
                        break;
                    }
                    offset += rows.len();
                }
                self.cursors
                    .lock()
                    .await
                    .insert(chat, before.saturating_sub(1));
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    pub(crate) async fn send(&self, id: i64, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let data = URL_SAFE_NO_PAD
            .decode(chat.strip_prefix("bb_").ok_or(Some(400))?)
            .map_err(|_| Some(400))?;
        let chat = String::from_utf8(data).map_err(|_| Some(400))?;
        if !self.cursors.lock().await.contains_key(&chat) {
            return Err(Some(403));
        }
        let endpoint = self
            .base
            .join("api/v1/message/text")
            .map_err(|_| Some(400))?;
        let response = self.http.post(endpoint).query(&[("password",&self.password)])
            .json(&json!({"chatGuid":chat,"message":text,"method":"apple-script","tempGuid":format!("kyler-{id}")}))
            .timeout(Duration::from_secs(135)).send().await.map_err(|_| None)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Some(response.status().as_u16()))
        }
    }
}

fn valid_chat(guid: &str) -> bool {
    guid.strip_prefix("iMessage;-;").is_some_and(|address| {
        !address.is_empty()
            && address.len() <= 240
            && address
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"+-._@".contains(&byte))
    })
}

fn message(chat: &str, after: i64, before: i64, row: &Value) -> Option<Incoming> {
    let date = row["dateCreated"].as_i64()?;
    if row["isFromMe"] != false || date <= after || date > before {
        return None;
    }
    let guid = row["guid"].as_str()?;
    let text = row["text"].as_str()?;
    if guid.is_empty() || text.trim().is_empty() {
        return None;
    }
    Some(Incoming {
        attachments: Vec::new(),
        update: format!("bb_{guid}"),
        chat: format!("bb_{}", URL_SAFE_NO_PAD.encode(chat)),
        text: text.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn processes_only_current_inbound_messages_in_configured_imessage_dm() {
        let chat = "iMessage;-;+12025550123";
        assert!(valid_chat(chat));
        assert!(!valid_chat("iMessage;+;chat987"));
        assert!(!valid_chat("SMS;-;+12025550123"));
        let mut row = json!({"dateCreated":2,"guid":"ab-123","isFromMe":false,"text":"hello"});
        assert!(message(chat, 1, 3, &row).is_some());
        assert!(message(chat, 2, 3, &row).is_none());
        row["isFromMe"] = json!(true);
        assert!(message(chat, 1, 3, &row).is_none());
    }
}
