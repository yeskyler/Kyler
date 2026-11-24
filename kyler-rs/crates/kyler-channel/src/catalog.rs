use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use serde::Serialize;

pub const WEB_ORIGIN_KEY: &str = "KYLER_CHANNEL_WEB_URL";

#[derive(Clone, Copy, Serialize)]
pub struct ChannelField {
    pub key: &'static str,
    pub label: &'static str,
    pub secret: bool,
    pub required: bool,
}

pub struct ChannelDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub fields: &'static [ChannelField],
}

const fn field(
    key: &'static str,
    label: &'static str,
    secret: bool,
    required: bool,
) -> ChannelField {
    ChannelField {
        key,
        label,
        secret,
        required,
    }
}

pub const CHANNELS: &[ChannelDefinition] = &[
    ChannelDefinition {
        id: "telegram",
        name: "Telegram",
        fields: &[field("KYLER_TELEGRAM_BOT_TOKEN", "Bot Token", true, true)],
    },
    ChannelDefinition {
        id: "slack",
        name: "Slack",
        fields: &[
            field("KYLER_SLACK_BOT_TOKEN", "Bot Token", true, true),
            field("KYLER_SLACK_SIGNING_SECRET", "Signing Secret", true, true),
            field("KYLER_SLACK_TEAM_ID", "Team ID", false, true),
            field("KYLER_SLACK_APP_ID", "App ID", false, true),
        ],
    },
    ChannelDefinition {
        id: "feishu",
        name: "Feishu",
        fields: &[
            field("KYLER_FEISHU_APP_ID", "App ID", false, true),
            field("KYLER_FEISHU_APP_SECRET", "App Secret", true, true),
            field(
                "KYLER_FEISHU_VERIFICATION_TOKEN",
                "Verification Token",
                true,
                true,
            ),
            field("KYLER_FEISHU_TENANT_KEY", "Tenant Key", false, true),
        ],
    },
    ChannelDefinition {
        id: "line",
        name: "LINE",
        fields: &[
            field("KYLER_LINE_ACCESS_TOKEN", "Access Token", true, true),
            field("KYLER_LINE_BOT_USER_ID", "Bot User ID", false, true),
            field("KYLER_LINE_CHANNEL_SECRET", "Channel Secret", true, true),
        ],
    },
    ChannelDefinition {
        id: "whatsapp",
        name: "WhatsApp",
        fields: &[
            field("KYLER_WHATSAPP_ACCESS_TOKEN", "Access Token", true, true),
            field(
                "KYLER_WHATSAPP_PHONE_NUMBER_ID",
                "Phone Number ID",
                false,
                true,
            ),
            field("KYLER_WHATSAPP_GRAPH_VERSION", "Graph Version", false, true),
            field("KYLER_WHATSAPP_APP_SECRET", "App Secret", true, true),
            field("KYLER_WHATSAPP_VERIFY_TOKEN", "Verify Token", true, true),
        ],
    },
    ChannelDefinition {
        id: "dingtalk",
        name: "DingTalk",
        fields: &[
            field("KYLER_DINGTALK_APP_KEY", "App Key", true, true),
            field("KYLER_DINGTALK_CORP_ID", "Corp ID", false, true),
            field("KYLER_DINGTALK_APP_SECRET", "App Secret", true, true),
            field("KYLER_DINGTALK_ROBOT_CODE", "Robot Code", false, true),
        ],
    },
    ChannelDefinition {
        id: "wecom",
        name: "WeCom",
        fields: &[
            field("KYLER_WECOM_BOT_ID", "Bot ID", false, true),
            field("KYLER_WECOM_BOT_SECRET", "Bot Secret", true, true),
        ],
    },
    ChannelDefinition {
        id: "wecom_callback",
        name: "WeCom Callback",
        fields: &[
            field("KYLER_WECOM_CALLBACK_CORP_ID", "Corp ID", false, true),
            field("KYLER_WECOM_CALLBACK_AGENT_ID", "Agent ID", false, true),
            field("KYLER_WECOM_CALLBACK_APP_SECRET", "App Secret", true, true),
            field("KYLER_WECOM_CALLBACK_TOKEN", "Token", true, true),
            field("KYLER_WECOM_CALLBACK_AES_KEY", "AES Key", true, true),
        ],
    },
    ChannelDefinition {
        id: "weixin",
        name: "Weixin",
        fields: &[
            field("KYLER_WEIXIN_BOT_TOKEN", "Bot Token", true, true),
            field("KYLER_WEIXIN_BOT_ID", "Bot ID", false, true),
            field("KYLER_WEIXIN_USER_ID", "User ID", false, true),
            field("KYLER_WEIXIN_URL", "URL", false, false),
        ],
    },
    ChannelDefinition {
        id: "qqbot",
        name: "QQ Bot",
        fields: &[
            field("KYLER_QQBOT_APP_ID", "App ID", false, true),
            field("KYLER_QQBOT_APP_SECRET", "App Secret", true, true),
        ],
    },
    ChannelDefinition {
        id: "discord",
        name: "Discord",
        fields: &[
            field("KYLER_DISCORD_BOT_TOKEN", "Bot Token", true, true),
            field(
                "KYLER_DISCORD_APPLICATION_ID",
                "Application ID",
                false,
                true,
            ),
            field("KYLER_DISCORD_PUBLIC_KEY", "Public Key", false, true),
        ],
    },
    ChannelDefinition {
        id: "signal",
        name: "Signal",
        fields: &[
            field("KYLER_SIGNAL_SOCKET", "Socket", false, true),
            field("KYLER_SIGNAL_ACCOUNT", "Account", false, true),
            field("KYLER_SIGNAL_ALLOWED_ACIS", "Allowed Acis", false, true),
        ],
    },
    ChannelDefinition {
        id: "mattermost",
        name: "Mattermost",
        fields: &[
            field("KYLER_MATTERMOST_BOT_TOKEN", "Bot Token", true, true),
            field("KYLER_MATTERMOST_URL", "URL", false, true),
            field("KYLER_MATTERMOST_BOT_USER_ID", "Bot User ID", false, true),
        ],
    },
    ChannelDefinition {
        id: "matrix",
        name: "Matrix",
        fields: &[
            field("KYLER_MATRIX_ACCESS_TOKEN", "Access Token", true, true),
            field("KYLER_MATRIX_URL", "URL", false, true),
            field("KYLER_MATRIX_BOT_USER_ID", "Bot User ID", false, true),
        ],
    },
    ChannelDefinition {
        id: "email",
        name: "Email (Gmail)",
        fields: &[
            field("KYLER_EMAIL_ADDRESS", "Address", false, true),
            field("KYLER_EMAIL_ALLOWED_FROM", "Allowed From", false, true),
            field("KYLER_EMAIL_MESSAGE_SECRET", "Message Secret", true, true),
            field(
                "KYLER_EMAIL_GOOGLE_CLIENT_ID",
                "Google Client ID",
                false,
                true,
            ),
            field(
                "KYLER_EMAIL_GOOGLE_CLIENT_SECRET",
                "Google Client Secret",
                true,
                true,
            ),
            field(
                "KYLER_EMAIL_GOOGLE_REFRESH_TOKEN",
                "Google Refresh Token",
                true,
                true,
            ),
        ],
    },
    ChannelDefinition {
        id: "sms",
        name: "SMS (Twilio)",
        fields: &[
            field(
                "KYLER_SMS_TWILIO_ACCOUNT_SID",
                "Twilio Account Sid",
                false,
                true,
            ),
            field(
                "KYLER_SMS_TWILIO_AUTH_TOKEN",
                "Twilio Auth Token",
                true,
                true,
            ),
            field("KYLER_SMS_TWILIO_NUMBER", "Twilio Number", false, true),
            field(
                "KYLER_SMS_TWILIO_WEBHOOK_URL",
                "Twilio Webhook URL",
                false,
                true,
            ),
            field("KYLER_SMS_ALLOWED_FROM", "Allowed From", false, true),
        ],
    },
    ChannelDefinition {
        id: "bluebubbles",
        name: "BlueBubbles",
        fields: &[
            field("KYLER_BLUEBUBBLES_PASSWORD", "Password", true, true),
            field("KYLER_BLUEBUBBLES_URL", "URL", false, true),
            field("KYLER_BLUEBUBBLES_CHATS", "Chats", false, true),
        ],
    },
    ChannelDefinition {
        id: "home_assistant",
        name: "Home Assistant",
        fields: &[
            field("KYLER_HOME_ASSISTANT_TOKEN", "Token", true, true),
            field("KYLER_HOME_ASSISTANT_URL", "URL", false, true),
            field("KYLER_HOME_ASSISTANT_USER_ID", "User ID", false, true),
        ],
    },
    ChannelDefinition {
        id: "teams",
        name: "Microsoft Teams",
        fields: &[
            field("KYLER_TEAMS_APP_ID", "App ID", false, true),
            field("KYLER_TEAMS_TENANT_ID", "Tenant ID", false, true),
            field("KYLER_TEAMS_CLIENT_SECRET", "Client Secret", true, true),
        ],
    },
    ChannelDefinition {
        id: "google_chat",
        name: "Google Chat",
        fields: &[
            field("KYLER_GOOGLE_CHAT_WEBHOOK_URL", "Webhook URL", false, true),
            field(
                "KYLER_GOOGLE_CHAT_CREDENTIALS_FILE",
                "Credentials File",
                false,
                true,
            ),
        ],
    },
    ChannelDefinition {
        id: "api_server",
        name: "API Server",
        fields: &[
            field("KYLER_CHANNEL_API_SERVER_SECRET", "Secret", true, true),
            field("KYLER_CHANNEL_API_SERVER_INSTANCE", "Instance", false, true),
        ],
    },
    ChannelDefinition {
        id: "webhook",
        name: "Webhook",
        fields: &[
            field("KYLER_CHANNEL_WEBHOOK_SECRET", "Secret", true, true),
            field("KYLER_CHANNEL_WEBHOOK_INSTANCE", "Instance", false, true),
            field("KYLER_CHANNEL_WEBHOOK_REPLY_URL", "Reply URL", false, true),
            field(
                "KYLER_CHANNEL_WEBHOOK_REPLY_SECRET",
                "Reply Secret",
                true,
                true,
            ),
        ],
    },
];

pub struct Settings<'a>(pub &'a BTreeMap<String, String>);

impl Settings<'_> {
    pub fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().or_else(|| self.0.get(key).cloned())
    }

    pub fn required(&self, key: &str) -> Result<String> {
        let value = self.get(key).unwrap_or_default();
        ensure!(!value.trim().is_empty(), "{key} is required");
        Ok(value)
    }

    pub fn web_origin(&self) -> Result<url::Url> {
        let raw = self.required(WEB_ORIGIN_KEY)?;
        let url = url::Url::parse(&raw).map_err(|_| anyhow::anyhow!("Invalid channel Web URL"))?;
        ensure!(
            matches!(url.scheme(), "http" | "https")
                && url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none(),
            "Channel Web URL must be an HTTP or HTTPS origin without credentials"
        );
        Ok(url)
    }
}
