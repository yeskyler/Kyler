use std::time::Duration;

use anyhow::Result;
use hmac::{Hmac, KeyInit, Mac};
use sha2_new::Sha256;

use crate::{
    CHANNELS, ChannelDefinition, Incoming, ReplyReceipt, Settings,
    adapters::{
        bluebubbles::BlueBubbles, bridge::Bridge, dingtalk::DingTalk, discord::Discord,
        email::Email, feishu::Feishu, google_chat::GoogleChat, home_assistant::HomeAssistant,
        line::Line, matrix::Matrix, mattermost::Mattermost, qqbot::QqBot, signal::Signal,
        slack::Slack, sms::Sms, teams::Teams, telegram::Telegram, wecom::WeCom,
        wecom_callback::WeComCallback, weixin::Weixin, whatsapp::WhatsApp,
    },
};

pub enum Adapter {
    Telegram(Telegram),
    Slack(Slack),
    Feishu(Feishu),
    Line(Line),
    WhatsApp(WhatsApp),
    Bridge(Bridge),
    Discord(Discord),
    DingTalk(DingTalk),
    Mattermost(Mattermost),
    Matrix(Matrix),
    Sms(Sms),
    HomeAssistant(HomeAssistant),
    BlueBubbles(BlueBubbles),
    Signal(Signal),
    WeComCallback(WeComCallback),
    WeCom(WeCom),
    QqBot(QqBot),
    Weixin(Weixin),
    Teams(Teams),
    GoogleChat(GoogleChat),
    Email(Email),
}

pub enum Webhook {
    Challenge(String),
    Messages(Vec<Incoming>),
    Contextual(Incoming, String),
    Ignore,
    Response(serde_json::Value, Vec<Incoming>),
    Xml(&'static str, Vec<Incoming>),
    Plain(&'static str, Vec<Incoming>),
}

pub(crate) fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()?)
}

impl Adapter {
    pub fn configured(settings: &Settings<'_>) -> Result<Vec<Self>> {
        CHANNELS
            .iter()
            .filter(|definition| settings.get(definition.fields[0].key).is_some())
            .map(|definition| Self::from_settings(definition, settings))
            .collect()
    }

    pub fn from_settings(definition: &ChannelDefinition, settings: &Settings<'_>) -> Result<Self> {
        Ok(match definition.id {
            "telegram" => Self::Telegram(Telegram::new(
                &settings.required(definition.fields[0].key)?,
            )?),
            "slack" => Self::Slack(Slack::from_settings(settings)?),
            "feishu" => Self::Feishu(Feishu::from_settings(settings)?),
            "line" => Self::Line(Line::from_settings(settings)?),
            "whatsapp" => Self::WhatsApp(WhatsApp::from_settings(settings)?),
            "dingtalk" => Self::DingTalk(DingTalk::from_settings(settings)?),
            "wecom" => Self::WeCom(WeCom::from_settings(settings)?),
            "wecom_callback" => Self::WeComCallback(WeComCallback::from_settings(settings)?),
            "weixin" => Self::Weixin(Weixin::from_settings(settings)?),
            "qqbot" => Self::QqBot(QqBot::from_settings(settings)?),
            "discord" => Self::Discord(Discord::from_settings(settings)?),
            "signal" => Self::Signal(Signal::from_settings(settings)?),
            "mattermost" => Self::Mattermost(Mattermost::from_settings(settings)?),
            "matrix" => Self::Matrix(Matrix::from_settings(settings)?),
            "email" => Self::Email(Email::from_settings(settings)?),
            "sms" => Self::Sms(Sms::from_settings(settings)?),
            "bluebubbles" => Self::BlueBubbles(BlueBubbles::from_settings(settings)?),
            "home_assistant" => Self::HomeAssistant(HomeAssistant::from_settings(settings)?),
            "teams" => Self::Teams(Teams::from_settings(settings)?),
            "google_chat" => Self::GoogleChat(GoogleChat::from_settings(settings)?),
            "api_server" => Self::Bridge(Bridge::from_settings(definition.id, settings)?),
            "webhook" => Self::Bridge(Bridge::from_settings(definition.id, settings)?),
            _ => anyhow::bail!("Unknown channel"),
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Telegram(_) => "telegram",
            Self::Slack(_) => "slack",
            Self::Feishu(_) => "feishu",
            Self::Line(_) => "line",
            Self::WhatsApp(_) => "whatsapp",
            Self::Bridge(bridge) => bridge.profile,
            Self::Discord(_) => "discord",
            Self::DingTalk(_) => "dingtalk",
            Self::Mattermost(_) => "mattermost",
            Self::Matrix(_) => "matrix",
            Self::Sms(_) => "sms",
            Self::HomeAssistant(_) => "home_assistant",
            Self::BlueBubbles(_) => "bluebubbles",
            Self::Signal(_) => "signal",
            Self::WeComCallback(_) => "wecom_callback",
            Self::WeCom(_) => "wecom",
            Self::QqBot(_) => "qqbot",
            Self::Weixin(_) => "weixin",
            Self::Teams(_) => "teams",
            Self::GoogleChat(_) => "google_chat",
            Self::Email(_) => "email",
        }
    }

    pub fn namespace(&self) -> String {
        match self {
            Self::Telegram(bot) => bot.bot_id.to_string(),
            Self::Slack(bot) => format!("slack:{}:{}", bot.team, bot.app),
            Self::Feishu(bot) => format!("feishu:{}:{}", bot.app, bot.tenant),
            Self::Line(bot) => format!("line:{}", bot.destination),
            Self::WhatsApp(bot) => format!("whatsapp:{}", bot.phone),
            Self::Bridge(bridge) => format!("bridge:{}:{}", bridge.profile, bridge.instance),
            Self::Discord(bot) => format!("discord:{}", bot.app),
            Self::DingTalk(bot) => format!("dingtalk:{}:{}", bot.corp, bot.app),
            Self::Mattermost(bot) => bot.namespace(),
            Self::Matrix(bot) => bot.namespace(),
            Self::Sms(bot) => format!("sms:{}:{}", bot.account, bot.number),
            Self::HomeAssistant(bot) => bot.namespace(),
            Self::BlueBubbles(bot) => bot.namespace(),
            Self::Signal(bot) => format!("signal:{}", bot.account),
            Self::WeComCallback(bot) => format!("wecom_callback:{}:{}", bot.corp, bot.agent),
            Self::WeCom(bot) => format!("wecom:{}", bot.bot),
            Self::QqBot(bot) => format!("qqbot:{}", bot.app),
            Self::Weixin(bot) => bot.namespace(),
            Self::Teams(bot) => bot.namespace(),
            Self::GoogleChat(bot) => bot.namespace(),
            Self::Email(bot) => bot.namespace(),
        }
    }

    pub async fn send(
        &self,
        id: i64,
        chat: &str,
        text: &str,
        receipt: Option<&ReplyReceipt>,
    ) -> Result<(), Option<u16>> {
        match self {
            Self::Telegram(bot) => bot.send(chat, text).await,
            Self::Slack(bot) => bot.send(chat, text).await,
            Self::Feishu(bot) => bot.send(chat, text).await,
            Self::Line(bot) => bot.send(chat, text).await,
            Self::WhatsApp(bot) => bot.send(chat, text).await,
            Self::Bridge(bridge) => bridge.send(id, chat, text).await,
            Self::Discord(bot) => bot.send(chat, text).await,
            Self::DingTalk(bot) => bot.send(chat, text).await,
            Self::Mattermost(bot) => bot.send(chat, text).await,
            Self::Matrix(bot) => bot.send(id, chat, text).await,
            Self::Sms(bot) => bot.send(chat, text).await,
            Self::HomeAssistant(bot) => bot.send(id, chat, text).await,
            Self::BlueBubbles(bot) => bot.send(id, chat, text).await,
            Self::Signal(bot) => bot.send(chat, text).await,
            Self::WeComCallback(bot) => bot.send(chat, text).await,
            Self::WeCom(bot) => bot.send(chat, text).await,
            Self::QqBot(bot) => bot.send(chat, text, receipt).await,
            Self::Weixin(bot) => bot.send(id, chat, text, receipt).await,
            Self::Teams(bot) => bot.send(chat, text, receipt).await,
            Self::GoogleChat(bot) => bot.send(id, chat, text).await,
            Self::Email(bot) => bot.send(id, chat, text).await,
        }
    }

    pub async fn webhook(
        &self,
        headers: &reqwest::header::HeaderMap,
        body: &[u8],
    ) -> Result<Webhook> {
        match self {
            Self::Slack(bot) => bot.webhook(headers, body),
            Self::Feishu(bot) => bot.webhook(body),
            Self::Line(bot) => bot.webhook(headers, body),
            Self::WhatsApp(bot) => bot.webhook(headers, body),
            Self::Bridge(bridge) => bridge.webhook(headers, body),
            Self::Discord(bot) => bot.webhook(headers, body),
            Self::Sms(bot) => bot.webhook(headers, body),
            Self::QqBot(bot) => bot.webhook(headers, body),
            Self::Teams(bot) => bot.webhook(headers, body).await,
            Self::GoogleChat(bot) => bot.webhook(headers, body).await,
            Self::WeComCallback(_) => anyhow::bail!("WeCom requires callback query parameters"),
            Self::Telegram(_)
            | Self::DingTalk(_)
            | Self::Mattermost(_)
            | Self::Matrix(_)
            | Self::HomeAssistant(_)
            | Self::BlueBubbles(_)
            | Self::Signal(_)
            | Self::WeCom(_)
            | Self::Weixin(_)
            | Self::Email(_) => {
                anyhow::bail!("Channel uses an outbound connection")
            }
        }
    }
    pub fn accepts_webhook(&self) -> bool {
        !matches!(self, Self::Telegram(_)) && !self.streams()
    }

    pub fn streams(&self) -> bool {
        matches!(
            self,
            Self::DingTalk(_)
                | Self::Mattermost(_)
                | Self::Matrix(_)
                | Self::HomeAssistant(_)
                | Self::BlueBubbles(_)
                | Self::Signal(_)
                | Self::WeCom(_)
                | Self::Weixin(_)
                | Self::Email(_)
        )
    }

    pub async fn listen(&self, store: &crate::ChannelStore) -> Result<()> {
        match self {
            Self::DingTalk(bot) => bot.listen(store).await,
            Self::Mattermost(bot) => bot.listen(store).await,
            Self::Matrix(bot) => bot.listen(store).await,
            Self::HomeAssistant(bot) => bot.listen(store).await,
            Self::BlueBubbles(bot) => bot.listen(store).await,
            Self::Signal(bot) => bot.listen(store).await,
            Self::WeCom(bot) => bot.listen(store).await,
            Self::Weixin(bot) => bot.listen(store).await,
            Self::Email(bot) => bot.listen(store).await,
            _ => anyhow::bail!("Channel does not use an outbound stream"),
        }
    }

    pub fn verify_subscription(&self, mode: &str, token: &str) -> bool {
        matches!(self, Self::WhatsApp(bot) if bot.verify_subscription(mode, token))
    }
}

pub(crate) fn verify_hmac(secret: &str, body: &[u8], signature: &[u8]) -> Result<()> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())?;
    mac.update(body);
    mac.verify_slice(signature)
        .map_err(|_| anyhow::anyhow!("Invalid webhook signature"))
}

pub(crate) fn fresh(timestamp: i64) -> bool {
    let now = chrono::Utc::now().timestamp();
    (now - 7 * 86400..=now + 300).contains(&timestamp)
}
