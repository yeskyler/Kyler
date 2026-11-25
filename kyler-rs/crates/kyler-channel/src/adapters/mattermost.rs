use std::time::Duration;

use anyhow::{Context, Result, ensure};
use futures::{SinkExt, StreamExt};
use reqwest::Url;
use serde_json::{Value, json};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

use crate::{ChannelStore, Incoming, Settings, adapter::client};

pub struct Mattermost {
    http: reqwest::Client,
    base: Url,
    token: String,
    user: String,
}

impl Mattermost {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let mut base = Url::parse(&settings.required("KYLER_MATTERMOST_URL")?)
            .map_err(|_| anyhow::anyhow!("Invalid Mattermost URL"))?;
        ensure!(
            base.scheme() == "https"
                && base.host().is_some()
                && base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none(),
            "Mattermost URL must be HTTPS without credentials, query, or fragment"
        );
        if !base.path().ends_with('/') {
            base.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("Invalid Mattermost base path"))?
                .push("");
        }
        let user = settings.required("KYLER_MATTERMOST_BOT_USER_ID")?;
        ensure!(valid_id(&user), "Invalid Mattermost bot user ID");
        Ok(Self {
            http: client()?,
            base,
            token: settings.required("KYLER_MATTERMOST_BOT_TOKEN")?,
            user,
        })
    }

    pub(crate) fn namespace(&self) -> String {
        format!("mattermost:{}:{}", self.base, self.user)
    }

    pub async fn listen(&self, store: &ChannelStore) -> Result<()> {
        let profile: Value = self
            .http
            .get(self.base.join("api/v4/users/me")?)
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        ensure!(
            profile["id"] == self.user && profile["is_bot"] == true,
            "Mattermost token does not identify the configured bot"
        );

        let mut url = self.base.join("api/v4/websocket")?;
        url.set_scheme("wss")
            .map_err(|_| anyhow::anyhow!("Invalid Mattermost WebSocket URL"))?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(256 * 1024))
            .max_frame_size(Some(256 * 1024));
        let (mut socket, _) = timeout(
            Duration::from_secs(15),
            connect_async_with_config(url.as_str(), Some(config), false),
        )
        .await??;
        let auth = json!({"seq":1,"action":"authentication_challenge","data":{"token":self.token}});
        timeout(
            Duration::from_secs(10),
            socket.send(Message::Text(auth.to_string().into())),
        )
        .await??;
        let mut authenticated = false;
        loop {
            let idle = if authenticated { 90 } else { 10 };
            let frame = match timeout(Duration::from_secs(idle), socket.next()).await {
                Ok(Some(frame)) => frame?,
                Ok(None) => return Ok(()),
                Err(_) if authenticated => {
                    timeout(
                        Duration::from_secs(10),
                        socket.send(Message::Ping(Vec::new().into())),
                    )
                    .await??;
                    timeout(Duration::from_secs(10), socket.next())
                        .await?
                        .context("Mattermost socket closed")??
                }
                Err(_) => anyhow::bail!("Mattermost WebSocket authentication timed out"),
            };
            let frame = match frame {
                Message::Text(text) => serde_json::from_str::<Value>(&text)?,
                Message::Close(_) => return Ok(()),
                Message::Ping(_) => {
                    timeout(Duration::from_secs(10), socket.flush()).await??;
                    continue;
                }
                _ => continue,
            };
            if frame["seq_reply"] == 1 {
                ensure!(
                    frame["status"] == "OK",
                    "Mattermost WebSocket authentication failed"
                );
                authenticated = true;
            }
            if authenticated && let Some(message) = incoming(&self.user, &frame) {
                store.ingest(vec![message]).await?;
            }
        }
    }

    pub(crate) async fn send(&self, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let chat = chat
            .strip_prefix("mm_")
            .filter(|id| valid_id(id))
            .ok_or(Some(400))?;
        let endpoint = self.base.join("api/v4/posts").map_err(|_| Some(400))?;
        let response = self
            .http
            .post(endpoint)
            .bearer_auth(&self.token)
            .json(&json!({"channel_id":chat,"message":text}))
            .send()
            .await
            .map_err(|_| None)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else if status.as_u16() == 404 {
            Err(Some(403))
        } else {
            Err(Some(status.as_u16()))
        }
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 26
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn incoming(bot: &str, frame: &Value) -> Option<Incoming> {
    let data = &frame["data"];
    if frame["event"] != "posted" || data["channel_type"] != "D" {
        return None;
    }
    let post: Value = serde_json::from_str(data["post"].as_str()?).ok()?;
    let user = post["user_id"].as_str()?;
    let chat = post["channel_id"].as_str()?;
    let id = post["id"].as_str()?;
    let text = post["message"].as_str()?;
    if user == bot
        || !valid_id(user)
        || !valid_id(chat)
        || !valid_id(id)
        || text.trim().is_empty()
        || post["type"].as_str().is_some_and(|kind| !kind.is_empty())
        || post["root_id"]
            .as_str()
            .is_some_and(|root| !root.is_empty())
        || post["props"]["from_bot"] == true
        || post["props"]["from_webhook"] == true
    {
        return None;
    }
    let (first, second) = data["channel_name"].as_str()?.split_once("__")?;
    if !((first == bot && second == user) || (first == user && second == bot)) {
        return None;
    }
    Some(Incoming {
        attachments: Vec::new(),
        update: format!("mm_{id}"),
        chat: format!("mm_{chat}"),
        text: text.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_group_thread_system_and_bot_posts() {
        let bot = "bbbbbbbbbbbbbbbbbbbbbbbbbb";
        let user = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
        let channel_name = format!("{user}__{bot}");
        for (kind, author, root, post_type, allowed) in [
            ("D", user, "", "", true),
            ("G", user, "", "", false),
            ("D", bot, "", "", false),
            ("D", user, "thread", "", false),
            ("D", user, "", "system_join_channel", false),
        ] {
            let post = json!({"id":"eeeeeeeeeeeeeeeeeeeeeeeeee","channel_id":"cccccccccccccccccccccccccc",
                "user_id":author,"root_id":root,"type":post_type,"message":"hello"});
            let wire = json!({"event":"posted","data":{"channel_type":kind,"channel_name":channel_name,"post":post.to_string()}});
            assert_eq!(incoming(bot, &wire).is_some(), allowed);
        }
    }
}
