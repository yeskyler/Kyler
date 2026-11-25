use std::time::Duration;

use anyhow::{Result, ensure};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD},
};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;

use super::cloud_auth::{AccessToken, digest, opaque_id};
use crate::{ChannelStore, Incoming, Settings, adapter::client};

const API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

pub struct Email {
    http: reqwest::Client,
    mailbox: String,
    sender: String,
    message_secret: String,
    client_id: String,
    client_secret: String,
    refresh_token: String,
    access: AccessToken,
}

impl Email {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let mailbox = settings
            .required("KYLER_EMAIL_ADDRESS")?
            .to_ascii_lowercase();
        let sender = settings
            .required("KYLER_EMAIL_ALLOWED_FROM")?
            .to_ascii_lowercase();
        let message_secret = settings.required("KYLER_EMAIL_MESSAGE_SECRET")?;
        ensure!(
            address(&mailbox) && address(&sender) && mailbox != sender,
            "Invalid Gmail mailbox or sender"
        );
        ensure!(
            message_secret.len() >= 32 && message_secret.bytes().all(|b| b.is_ascii_graphic()),
            "Email message secret must contain at least 32 printable non-space ASCII characters"
        );
        Ok(Self {
            http: client()?,
            mailbox,
            sender,
            message_secret,
            client_id: settings.required("KYLER_EMAIL_GOOGLE_CLIENT_ID")?,
            client_secret: settings.required("KYLER_EMAIL_GOOGLE_CLIENT_SECRET")?,
            refresh_token: settings.required("KYLER_EMAIL_GOOGLE_REFRESH_TOKEN")?,
            access: AccessToken::default(),
        })
    }

    pub(crate) fn namespace(&self) -> String {
        format!("email:{}:{}", self.mailbox, self.sender)
    }

    fn chat(&self) -> String {
        format!("email_{}", digest(&self.sender))
    }

    async fn token(&self) -> Result<String, Option<u16>> {
        self.access
            .get(&self.http, "https://oauth2.googleapis.com/token", || {
                Ok(serde_urlencoded::to_string([
                    ("grant_type", "refresh_token"),
                    ("client_id", self.client_id.as_str()),
                    ("client_secret", self.client_secret.as_str()),
                    ("refresh_token", self.refresh_token.as_str()),
                ])?)
            })
            .await
    }

    async fn checked(
        &self,
        request: reqwest::RequestBuilder,
        token: &str,
    ) -> Result<reqwest::Response, Option<u16>> {
        let response = request.bearer_auth(token).send().await.map_err(|_| None)?;
        self.access.check(response, token).await
    }

    async fn verify_mailbox(&self, token: &str) -> Result<(), Option<u16>> {
        let profile = self
            .checked(self.http.get(format!("{API}/profile")), token)
            .await?
            .json::<Value>()
            .await
            .map_err(|_| None)?;
        if !profile["emailAddress"]
            .as_str()
            .is_some_and(|value| value.eq_ignore_ascii_case(&self.mailbox))
        {
            return Err(Some(403));
        }
        Ok(())
    }

    pub(crate) async fn listen(&self, store: &ChannelStore) -> Result<()> {
        loop {
            self.poll(store).await?;
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    }

    async fn poll(&self, store: &ChannelStore) -> Result<()> {
        let failure = |status: Option<u16>| {
            anyhow::anyhow!("Gmail channel request failed (status {status:?})")
        };
        let token = self.token().await.map_err(failure)?;
        self.verify_mailbox(&token).await.map_err(failure)?;
        let query = format!("is:unread in:inbox subject:Kyler from:{}", self.sender);
        let mut page = String::new();
        let mut accepted = Vec::new();
        loop {
            let mut url = reqwest::Url::parse(&format!("{API}/messages"))?;
            url.query_pairs_mut()
                .append_pair("q", &query)
                .append_pair("maxResults", "50");
            if !page.is_empty() {
                url.query_pairs_mut().append_pair("pageToken", &page);
            }
            let result = self
                .checked(self.http.get(url), &token)
                .await
                .map_err(failure)?
                .json::<Value>()
                .await?;
            if let Some(messages) = result["messages"].as_array() {
                for entry in messages {
                    let Some(id) = entry["id"].as_str().filter(|id| opaque_id(id)) else {
                        continue;
                    };
                    let url = format!("{API}/messages/{id}");
                    let response = match self
                        .checked(self.http.get(&url).query(&[("format", "full")]), &token)
                        .await
                    {
                        Ok(response) => response,
                        Err(Some(404)) => continue,
                        Err(status) => return Err(failure(status)),
                    };
                    let message = response.json::<Value>().await?;
                    if let Some(text) = command(&message, &self.sender, &self.message_secret) {
                        store
                            .ingest(vec![Incoming {
                                attachments: Vec::new(),
                                update: format!("gmail_{id}"),
                                chat: self.chat(),
                                text,
                            }])
                            .await?;
                        accepted.push(url);
                    }
                }
            }
            let Some(next) = result["nextPageToken"]
                .as_str()
                .filter(|next| !next.is_empty() && *next != page)
            else {
                break;
            };
            page = next.to_owned();
        }
        for url in accepted {
            match self
                .checked(
                    self.http
                        .post(format!("{url}/modify"))
                        .json(&json!({"removeLabelIds":["UNREAD"]})),
                    &token,
                )
                .await
            {
                Ok(_) | Err(Some(404)) => {}
                Err(status) => return Err(failure(status)),
            }
        }
        Ok(())
    }

    pub(crate) async fn send(&self, id: i64, chat: &str, text: &str) -> Result<(), Option<u16>> {
        if chat != self.chat() {
            return Err(Some(403));
        }
        let token = self.token().await?;
        self.verify_mailbox(&token).await?;
        let raw = mime(id, &self.mailbox, &self.sender, text);
        self.checked(
            self.http
                .post(format!("{API}/messages/send"))
                .json(&json!({"raw":URL_SAFE_NO_PAD.encode(raw)})),
            &token,
        )
        .await?;
        Ok(())
    }
}

fn address(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    value.len() <= 254
        && !local.is_empty()
        && domain.contains('.')
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
        && domain
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
}

fn command(message: &Value, sender: &str, secret: &str) -> Option<String> {
    let labels = message["labelIds"].as_array()?;
    if !labels.iter().any(|label| label == "INBOX") || !labels.iter().any(|label| label == "UNREAD")
    {
        return None;
    }
    let payload = &message["payload"];
    let headers = payload["headers"].as_array()?;
    let header = |name: &str| {
        let mut matched = headers.iter().filter(|entry| {
            entry["name"]
                .as_str()
                .is_some_and(|key| key.eq_ignore_ascii_case(name))
        });
        let value = matched.next()?["value"].as_str()?;
        matched.next().is_none().then_some(value.trim())
    };
    let from = header("From")?;
    let from = from
        .strip_suffix('>')
        .and_then(|from| from.rsplit_once('<'))
        .map_or(from, |(_, email)| email);
    if !from.eq_ignore_ascii_case(sender) || !header("Subject")?.eq_ignore_ascii_case("Kyler") {
        return None;
    }
    let raw = plain_text(payload)?;
    let (authorization, text) = raw.split_once('\n')?;
    let supplied = authorization
        .trim_end_matches('\r')
        .strip_prefix("Kyler-Auth: ")?;
    let text = text.trim();
    if !bool::from(supplied.as_bytes().ct_eq(secret.as_bytes())) || text.is_empty() {
        return None;
    }
    Some(text.to_owned())
}

fn plain_text(part: &Value) -> Option<String> {
    if part["filename"]
        .as_str()
        .is_some_and(|name| !name.is_empty())
    {
        return None;
    }
    match part["mimeType"].as_str()? {
        "text/plain" => {
            let data = part["body"]["data"].as_str()?;
            if data.len() > 262_144 {
                return None;
            }
            String::from_utf8(
                URL_SAFE_NO_PAD
                    .decode(data)
                    .or_else(|_| URL_SAFE.decode(data))
                    .ok()?,
            )
            .ok()
        }
        "multipart/alternative" | "multipart/mixed" | "multipart/related" => {
            part["parts"].as_array()?.iter().find_map(plain_text)
        }
        _ => None,
    }
}

fn mime(id: i64, mailbox: &str, sender: &str, text: &str) -> String {
    let mut message = format!(
        "From: {mailbox}\r\nTo: {sender}\r\nSubject: Re: Kyler\r\nMessage-ID: <kyler-{id}@{}>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n",
        mailbox
            .split_once('@')
            .map_or("localhost", |(_, domain)| domain)
    );
    let encoded = STANDARD.encode(text);
    for start in (0..encoded.len()).step_by(76) {
        message.push_str(&encoded[start..(start + 76).min(encoded.len())]);
        message.push_str("\r\n");
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_authentication_and_does_not_forward_the_secret() {
        let sender = "user@example.com";
        let secret = "test-secret";
        let event = json!({"labelIds":["UNREAD","INBOX"],"payload":{
            "mimeType":"text/plain","headers":[{"name":"From","value":"User <user@example.com>"},{"name":"Subject","value":"Kyler"}],
            "body":{"data":URL_SAFE_NO_PAD.encode("Kyler-Auth: test-secret\r\nhello")}
        }});
        assert_eq!(command(&event, sender, secret).as_deref(), Some("hello"));
        assert!(command(&event, sender, "wrong-secret").is_none());
        assert!(command(&event, "other@example.com", secret).is_none());
        assert!(mime(1, "bot@example.com", sender, "你好").ends_with("5L2g5aW9\r\n"));
    }
}
