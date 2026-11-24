use std::collections::{BTreeMap, HashSet};

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, KeyInit, Mac};
use reqwest::header::HeaderMap;
use sha1::Sha1;
use url::{Url, form_urlencoded};

use crate::{Incoming, Settings, Webhook, adapter::client};

pub struct Sms {
    http: reqwest::Client,
    pub(crate) account: String,
    pub(crate) number: String,
    secret: String,
    callback: String,
    allow: HashSet<String>,
}

impl Sms {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let account = settings.required("KYLER_SMS_TWILIO_ACCOUNT_SID")?;
        let number = settings.required("KYLER_SMS_TWILIO_NUMBER")?;
        let callback = settings.required("KYLER_SMS_TWILIO_WEBHOOK_URL")?;
        let url =
            Url::parse(&callback).map_err(|_| anyhow::anyhow!("Invalid Twilio webhook URL"))?;
        ensure!(
            sid(&account, "AC") && phone(&number),
            "Invalid Twilio account SID or SMS number"
        );
        ensure!(
            url.scheme() == "https"
                && url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path().ends_with("/webhooks/channels/sms"),
            "Twilio webhook URL must be an HTTPS SMS callback without credentials, query, or fragment"
        );
        let allow = settings
            .required("KYLER_SMS_ALLOWED_FROM")?
            .split(',')
            .map(|number| number.trim().to_owned())
            .collect::<HashSet<_>>();
        ensure!(
            allow.iter().all(|value| phone(value) && value != &number),
            "Invalid SMS sender allowlist"
        );
        Ok(Self {
            http: client()?,
            account,
            number,
            callback,
            allow,
            secret: settings.required("KYLER_SMS_TWILIO_AUTH_TOKEN")?,
        })
    }

    pub(crate) fn webhook(&self, headers: &HeaderMap, body: &[u8]) -> Result<Webhook> {
        let signature = headers
            .get("x-twilio-signature")
            .context("Missing Twilio signature")?
            .as_bytes();
        let params = signed_params(&self.callback, &self.secret, signature, body)?;
        let messages = match (
            params.get("MessageSid"),
            params.get("From"),
            params.get("Body"),
        ) {
            (Some(id), Some(from), Some(text))
                if sid(id, "SM")
                    && self.allow.contains(from)
                    && params.get("To") == Some(&self.number)
                    && params.get("AccountSid") == Some(&self.account)
                    && params.get("NumMedia").is_some_and(|count| count == "0")
                    && !text.trim().is_empty() =>
            {
                vec![Incoming {
                    update: id.to_owned(),
                    chat: format!("sms_{}", &from[1..]),
                    text: text.to_owned(),
                }]
            }
            _ => Vec::new(),
        };
        Ok(Webhook::Xml("<Response/>", messages))
    }

    pub(crate) async fn send(&self, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let number = format!("+{}", chat.strip_prefix("sms_").ok_or(Some(400))?);
        if !self.allow.contains(&number) {
            return Err(Some(403));
        }
        let body = form_urlencoded::Serializer::new(String::new())
            .append_pair("From", &self.number)
            .append_pair("To", &number)
            .append_pair("Body", text)
            .finish();
        let response = self
            .http
            .post(format!(
                "https://api.twilio.com/2010-04-01/Accounts/{}/Messages.json",
                self.account
            ))
            .basic_auth(&self.account, Some(&self.secret))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|_| None)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Some(response.status().as_u16()))
        }
    }
}

fn phone(number: &str) -> bool {
    number.strip_prefix('+').is_some_and(|number| {
        (2..=15).contains(&number.len())
            && !number.starts_with('0')
            && number.bytes().all(|b| b.is_ascii_digit())
    })
}

fn sid(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix)
        .is_some_and(|rest| rest.len() == 32 && rest.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn signed_params(
    callback: &str,
    secret: &str,
    signature: &[u8],
    body: &[u8],
) -> Result<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();
    for (key, value) in form_urlencoded::parse(body) {
        ensure!(
            fields
                .insert(key.into_owned(), value.into_owned())
                .is_none(),
            "Duplicate Twilio form field"
        );
    }
    let mut mac = Hmac::<Sha1>::new_from_slice(secret.as_bytes())?;
    mac.update(callback.as_bytes());
    for (key, value) in &fields {
        mac.update(key.as_bytes());
        mac.update(value.as_bytes());
    }
    mac.verify_slice(&STANDARD.decode(signature)?)
        .map_err(|_| anyhow::anyhow!("Invalid Twilio signature"))?;
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_sorted_form_at_the_configured_public_url() -> Result<()> {
        let url = "https://host.example/webhooks/channels/sms";
        let mut mac = Hmac::<Sha1>::new_from_slice(b"key")?;
        mac.update(format!("{url}Bodyhello worldFrom+12025550123").as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());
        let body = b"From=%2B12025550123&Body=hello+world";
        assert_eq!(
            signed_params(url, "key", signature.as_bytes(), body)?["Body"],
            "hello world"
        );
        assert!(signed_params("https://other.example", "key", signature.as_bytes(), body).is_err());
        assert!(signed_params(url, "key", signature.as_bytes(), b"Body=x&Body=y").is_err());
        Ok(())
    }
}
