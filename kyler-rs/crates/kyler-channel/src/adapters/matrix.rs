use std::time::Duration;

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::Url;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{ChannelStore, Incoming, Settings, adapter::client};

pub struct Matrix {
    http: reqwest::Client,
    base: Url,
    user: String,
    token: String,
    cursor: Mutex<Option<String>>,
}

impl Matrix {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let mut base = Url::parse(&settings.required("KYLER_MATRIX_URL")?)
            .map_err(|_| anyhow::anyhow!("Invalid Matrix homeserver URL"))?;
        ensure!(
            base.scheme() == "https"
                && base.host().is_some()
                && base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none(),
            "Matrix homeserver URL must be HTTPS without credentials, query or fragment"
        );
        if !base.path().ends_with('/') {
            base.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("Invalid Matrix base path"))?
                .push("");
        }
        let user = settings.required("KYLER_MATRIX_BOT_USER_ID")?;
        ensure!(identifier(&user, '@'), "Invalid Matrix bot user ID");
        Ok(Self {
            http: client()?,
            base,
            user,
            token: settings.required("KYLER_MATRIX_ACCESS_TOKEN")?,
            cursor: Mutex::new(None),
        })
    }

    pub(crate) fn namespace(&self) -> String {
        format!("matrix:{}:{}", self.base, self.user)
    }

    fn endpoint(&self, tail: &[&str]) -> Result<Url> {
        let mut url = self.base.join("_matrix/client/v3/")?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Invalid Matrix API URL"))?
            .pop_if_empty()
            .extend(tail.iter().copied());
        Ok(url)
    }

    async fn get(&self, tail: &[&str]) -> Result<reqwest::Response> {
        Ok(self
            .http
            .get(self.endpoint(tail)?)
            .bearer_auth(&self.token)
            .send()
            .await?)
    }

    async fn room_allowed(&self, room: &str, peer: &str) -> Result<bool> {
        let response = self.get(&["rooms", room, "joined_members"]).await?;
        if matches!(response.status().as_u16(), 403 | 404) {
            return Ok(false);
        }
        let members: Value = response.error_for_status()?.json().await?;
        if !members["joined"].as_object().is_some_and(|joined| {
            joined.len() == 2 && joined.contains_key(&self.user) && joined.contains_key(peer)
        }) {
            return Ok(false);
        }
        let response = self.get(&["rooms", room, "state"]).await?;
        if matches!(response.status().as_u16(), 403 | 404) {
            return Ok(false);
        }
        let state: Vec<Value> = response.error_for_status()?.json().await?;
        Ok(private_state(&state))
    }

    pub async fn listen(&self, store: &ChannelStore) -> Result<()> {
        let profile: Value = self
            .get(&["account", "whoami"])
            .await?
            .error_for_status()?
            .json()
            .await?;
        ensure!(
            profile["user_id"] == self.user,
            "Matrix token does not identify the configured bot"
        );
        let filter = json!({"presence":{"types":[]},"account_data":{"types":[]},"room":{
            "ephemeral":{"types":[]},"account_data":{"types":[]},"state":{"types":[]},
            "timeline":{"types":["m.room.message"],"limit":100}}})
        .to_string();
        let mut cursor = self.cursor.lock().await;
        loop {
            let mut sync = self
                .http
                .get(self.endpoint(&["sync"])?)
                .bearer_auth(&self.token)
                .timeout(Duration::from_secs(40))
                .query(&[("filter", filter.as_str()), ("set_presence", "offline")]);
            if let Some(since) = cursor.as_deref() {
                sync = sync.query(&[("since", since), ("timeout", "25000")]);
            }
            let response: Value = sync.send().await?.error_for_status()?.json().await?;
            let next = response["next_batch"]
                .as_str()
                .context("Missing Matrix sync cursor")?;
            if cursor.is_some()
                && let Some(rooms) = response["rooms"]["join"].as_object()
            {
                for (room, data) in rooms {
                    if !identifier(room, '!') {
                        continue;
                    }
                    let Some(events) = data["timeline"]["events"].as_array() else {
                        continue;
                    };
                    let mut messages = Vec::new();
                    for event in events {
                        let Some((peer, message)) = incoming(&self.user, room, event) else {
                            continue;
                        };
                        if self.room_allowed(room, peer).await? {
                            messages.push(message);
                        }
                    }
                    store.ingest(messages).await?;
                }
            }
            *cursor = Some(next.into());
        }
    }

    pub(crate) async fn send(&self, id: i64, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let (room, peer) = decode_chat(chat).ok_or(Some(400))?;
        if !self.room_allowed(&room, &peer).await.map_err(|_| None)? {
            return Err(Some(403));
        }
        let txn = format!("kyler_{id}");
        let endpoint = self
            .endpoint(&["rooms", &room, "send", "m.room.message", &txn])
            .map_err(|_| Some(400))?;
        let result = self
            .http
            .put(endpoint)
            .bearer_auth(&self.token)
            .json(&json!({"msgtype":"m.text","body":text}))
            .send()
            .await
            .map_err(|_| None)?;
        if result.status().is_success() {
            Ok(())
        } else {
            Err(Some(result.status().as_u16()))
        }
    }
}

fn identifier(value: &str, prefix: char) -> bool {
    value.starts_with(prefix)
        && value.len() > 1
        && value.len() <= 255
        && !value
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
}

fn private_state(events: &[Value]) -> bool {
    let has = |kind: &str, field: &str, value: &str| {
        events.iter().any(|event| {
            event["type"] == kind && event["state_key"] == "" && event["content"][field] == value
        })
    };
    !events
        .iter()
        .any(|event| event["type"] == "m.room.encryption")
        && has("m.room.join_rules", "join_rule", "invite")
        && has("m.room.history_visibility", "history_visibility", "joined")
}

fn decode_chat(chat: &str) -> Option<(String, String)> {
    let data = URL_SAFE_NO_PAD.decode(chat.strip_prefix("mx_")?).ok()?;
    let text = String::from_utf8(data).ok()?;
    let (room, peer) = text.split_once('\n')?;
    (identifier(room, '!') && identifier(peer, '@')).then(|| (room.into(), peer.into()))
}

fn incoming<'a>(bot: &str, room: &str, event: &'a Value) -> Option<(&'a str, Incoming)> {
    let sender = event["sender"].as_str()?;
    let update = event["event_id"].as_str()?;
    let content = &event["content"];
    let text = content["body"].as_str()?;
    if sender == bot
        || !identifier(sender, '@')
        || !identifier(update, '$')
        || text.trim().is_empty()
        || event["type"] != "m.room.message"
        || content["msgtype"] != "m.text"
        || !content["m.relates_to"].is_null()
    {
        return None;
    }
    let chat = format!("mx_{}", URL_SAFE_NO_PAD.encode(format!("{room}\n{sender}")));
    Some((
        sender,
        Incoming {
            update: update.into(),
            chat,
            text: text.into(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_keeps_both_room_and_peer_and_requires_private_state() {
        let peer = "@user:example.org";
        let room = "!room:example.org";
        let mut event = json!({"sender":peer,"event_id":"$id","type":"m.room.message","content":{"body":"hello","msgtype":"m.text"}});
        let (_, message) = incoming("@bot:example.org", room, &event).unwrap();
        assert_eq!(decode_chat(&message.chat), Some((room.into(), peer.into())));
        assert!(incoming(peer, room, &event).is_none());
        event["content"]["m.relates_to"] = json!({"rel_type":"m.replace"});
        assert!(incoming("@bot:example.org", room, &event).is_none());
        let mut state = vec![
            json!({"type":"m.room.join_rules","state_key":"","content":{"join_rule":"invite"}}),
            json!({"type":"m.room.history_visibility","state_key":"","content":{"history_visibility":"joined"}}),
        ];
        assert!(private_state(&state));
        state.push(json!({"type":"m.room.encryption"}));
        assert!(!private_state(&state));
        assert!(!private_state(&[]));
    }
}
