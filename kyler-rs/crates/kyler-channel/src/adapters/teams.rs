use anyhow::{Context, Result, ensure};
use reqwest::{Url, header::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::cloud_auth::{AccessToken, Keys, digest};
use crate::{Incoming, ReplyReceipt, Settings, Webhook, adapter::client};

pub struct Teams {
    http: reqwest::Client,
    app: String,
    tenant: String,
    secret: String,
    keys: Keys,
    access: AccessToken,
}

#[derive(Serialize, Deserialize)]
struct Route {
    service: String,
    conversation: String,
    user: String,
    bot: String,
}

impl Route {
    fn chat(&self) -> String {
        format!(
            "teams_{}",
            digest(&json!([self.conversation, self.user]).to_string())
        )
    }

    fn reply_url(&self) -> Result<Url> {
        let mut url = Url::parse(&self.service)?;
        ensure!(
            url.scheme() == "https"
                && url.host_str() == Some("smba.trafficmanager.net")
                && url.port().is_none()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "Invalid Teams connector URL"
        );
        ensure!(id(&self.conversation), "Invalid Teams conversation ID");
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("Invalid Teams connector path"))?
            .pop_if_empty()
            .extend(["v3", "conversations", &self.conversation, "activities"]);
        Ok(url)
    }
}

fn id(value: &str) -> bool {
    (1..=512).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_graphic())
        && !matches!(value, "." | "..")
}

fn guid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

impl Teams {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let app = settings.required("KYLER_TEAMS_APP_ID")?;
        let tenant = settings.required("KYLER_TEAMS_TENANT_ID")?;
        ensure!(
            guid(&app) && guid(&tenant),
            "Invalid Teams application or tenant ID"
        );
        Ok(Self {
            http: client()?,
            app,
            tenant,
            secret: settings.required("KYLER_TEAMS_CLIENT_SECRET")?,
            keys: Keys::default(),
            access: AccessToken::default(),
        })
    }

    pub(crate) fn namespace(&self) -> String {
        format!("teams:{}:{}", self.tenant, self.app)
    }

    pub(crate) async fn webhook(&self, headers: &HeaderMap, body: &[u8]) -> Result<Webhook> {
        let claims = self
            .keys
            .verify(
                &self.http,
                "https://login.botframework.com/v1/.well-known/keys",
                headers,
                Some("msteams"),
            )
            .await?;
        ensure!(
            claims["iss"] == "https://api.botframework.com" && claims["aud"] == self.app,
            "Invalid Teams callback identity"
        );
        let event: Value = serde_json::from_slice(body)?;
        let service = event["serviceUrl"]
            .as_str()
            .context("Missing Teams connector URL")?;
        ensure!(
            claims["serviceurl"] == service && event["channelId"] == "msteams",
            "Invalid Teams connector identity"
        );
        if event["type"] != "message" || event["conversation"]["conversationType"] != "personal" {
            return Ok(Webhook::Ignore);
        }
        let tenant = event
            .pointer("/channelData/tenant/id")
            .and_then(Value::as_str)
            .or_else(|| {
                event
                    .pointer("/conversation/tenantId")
                    .and_then(Value::as_str)
            });
        ensure!(tenant == Some(self.tenant.as_str()), "Invalid Teams tenant");
        let field = |path| {
            event
                .pointer(path)
                .and_then(Value::as_str)
                .filter(|value| id(value))
                .context("Invalid Teams activity ID")
        };
        let route = Route {
            service: service.to_owned(),
            conversation: field("/conversation/id")?.to_owned(),
            user: field("/from/id")?.to_owned(),
            bot: field("/recipient/id")?.to_owned(),
        };
        route.reply_url()?;
        let Some(text) = event["text"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
        else {
            return Ok(Webhook::Ignore);
        };
        let update = format!(
            "teams_{}",
            digest(&json!([route.conversation, field("/id")?]).to_string())
        );
        Ok(Webhook::Contextual(
            Incoming {
                update,
                chat: route.chat(),
                text: text.to_owned(),
            },
            serde_json::to_string(&route)?,
        ))
    }

    pub(crate) async fn send(
        &self,
        chat: &str,
        text: &str,
        receipt: Option<&ReplyReceipt>,
    ) -> Result<(), Option<u16>> {
        let context = receipt
            .and_then(|r| r.context_token.as_deref())
            .ok_or(Some(400))?;
        let route: Route = serde_json::from_str(context).map_err(|_| Some(400))?;
        if chat != route.chat() || !id(&route.bot) || !id(&route.user) {
            return Err(Some(403));
        }
        let url = route.reply_url().map_err(|_| Some(400))?;
        let endpoint = format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            self.tenant
        );
        let token = self
            .access
            .get(&self.http, &endpoint, || {
                Ok(serde_urlencoded::to_string([
                    ("client_id", self.app.as_str()),
                    ("client_secret", self.secret.as_str()),
                    ("grant_type", "client_credentials"),
                    ("scope", "https://api.botframework.com/.default"),
                ])?)
            })
            .await?;
        let response = self.http.post(url).bearer_auth(&token).json(&json!({
            "type":"message", "text":text, "textFormat":"plain",
            "from":{"id":route.bot}, "recipient":{"id":route.user}, "conversation":{"id":route.conversation},
        })).send().await.map_err(|_| None)?;
        self.access.check(response, &token).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_only_to_public_connector_and_encodes_conversation() -> Result<()> {
        let mut route = Route {
            service: "https://smba.trafficmanager.net/teams/".into(),
            conversation: "a/b".into(),
            user: "person".into(),
            bot: "bot".into(),
        };
        assert_eq!(
            route.reply_url()?.as_str(),
            "https://smba.trafficmanager.net/teams/v3/conversations/a%2Fb/activities"
        );
        route.service = "https://smba.trafficmanager.net.attacker.example/teams/".into();
        assert!(route.reply_url().is_err());
        Ok(())
    }
}
