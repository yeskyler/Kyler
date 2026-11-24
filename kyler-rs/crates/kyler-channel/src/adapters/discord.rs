use anyhow::{Context, Result, ensure};
use reqwest::header::HeaderMap;
use ring::signature::{ED25519, UnparsedPublicKey};
use serde_json::{Value, json};

use crate::{Incoming, Settings, Webhook, adapter::client};

pub struct Discord {
    pub(crate) app: String,
    key: [u8; 32],
    token: String,
    http: reqwest::Client,
}

impl Discord {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let app = settings.required("KYLER_DISCORD_APPLICATION_ID")?;
        ensure!(snowflake(&app), "Invalid Discord application ID");
        let public_key = settings.required("KYLER_DISCORD_PUBLIC_KEY")?;
        let mut key = [0; 32];
        hex::decode_to_slice(public_key, &mut key).context("Invalid Discord public key")?;
        Ok(Self {
            app,
            key,
            token: settings.required("KYLER_DISCORD_BOT_TOKEN")?,
            http: client()?,
        })
    }

    pub(crate) fn webhook(&self, headers: &HeaderMap, body: &[u8]) -> Result<Webhook> {
        let timestamp = headers
            .get("x-signature-timestamp")
            .context("Missing Discord timestamp")?
            .to_str()?;
        ensure!(
            timestamp
                .parse::<i64>()?
                .abs_diff(chrono::Utc::now().timestamp())
                <= 300,
            "Expired Discord timestamp"
        );
        let mut signature = [0; 64];
        let raw = headers
            .get("x-signature-ed25519")
            .context("Missing Discord signature")?;
        hex::decode_to_slice(raw.as_bytes(), &mut signature)?;
        let signed = [timestamp.as_bytes(), body].concat();
        UnparsedPublicKey::new(&ED25519, self.key)
            .verify(&signed, &signature)
            .map_err(|_| anyhow::anyhow!("Invalid Discord signature"))?;
        decode(&self.app, body)
    }

    pub(crate) async fn send(&self, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let channel = chat
            .strip_prefix("d_")
            .filter(|id| snowflake(id))
            .ok_or(Some(400))?;
        let response = self
            .http
            .post(format!(
                "https://discord.com/api/v10/channels/{channel}/messages"
            ))
            .header("authorization", format!("Bot {}", self.token))
            .json(&json!({"content":text,"allowed_mentions":{"parse":[]}}))
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

fn snowflake(id: &str) -> bool {
    id.len() <= 20
        && id.parse::<u64>().is_ok_and(|value| value > 0)
        && id.bytes().all(|byte| byte.is_ascii_digit())
}

fn decode(app: &str, body: &[u8]) -> Result<Webhook> {
    let interaction: Value = serde_json::from_slice(body)?;
    if interaction["type"] == 1 {
        return Ok(Webhook::Response(json!({"type":1}), Vec::new()));
    }
    ensure!(
        interaction["application_id"] == app,
        "Unexpected Discord application"
    );
    let command = &interaction["data"];
    if interaction["type"] != 2
        || interaction["context"] != 1
        || !interaction["guild_id"].is_null()
        || interaction["channel"]["type"] != 1
        || command["name"] != "kyler"
        || command["type"] != 1
        || interaction["user"]["bot"] == true
        || !interaction["user"]["id"].as_str().is_some_and(snowflake)
    {
        return Ok(Webhook::Response(
            json!({"type":4,"data":{"content":"Use /kyler in a private chat with this bot."}}),
            Vec::new(),
        ));
    }
    let id = interaction["id"]
        .as_str()
        .filter(|id| snowflake(id))
        .context("Missing Discord interaction ID")?;
    let channel = interaction["channel"]["id"]
        .as_str()
        .filter(|id| snowflake(id))
        .context("Missing Discord channel ID")?;
    let text = command["options"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|item| item["name"] == "text" && item["type"] == 3)
        .and_then(|item| item["value"].as_str())
        .filter(|text| !text.trim().is_empty())
        .context("Missing Discord command text")?;
    Ok(Webhook::Response(
        json!({"type":4,"data":{"content":"Received. Kyler will reply here."}}),
        vec![Incoming {
            update: format!("d_{id}"),
            chat: format!("d_{channel}"),
            text: text.into(),
        }],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_private_bot_commands_are_queued() -> Result<()> {
        let mut event = json!({"type":2,"context":1,"application_id":"10","id":"11","user":{"id":"12"},
            "channel":{"type":1,"id":"13"},"data":{"type":1,"name":"kyler","options":[{"name":"text","type":3,"value":"hello"}]}});
        assert!(
            matches!(decode("10", &serde_json::to_vec(&event)?)?, Webhook::Response(_, items) if items.len() == 1 && items[0].chat == "d_13")
        );
        event["guild_id"] = json!("20");
        assert!(
            matches!(decode("10", &serde_json::to_vec(&event)?)?, Webhook::Response(_, items) if items.is_empty())
        );
        assert!(decode("99", &serde_json::to_vec(&event)?).is_err());
        Ok(())
    }
}
