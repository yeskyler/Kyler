# Channels

Kyler can receive private text messages from Telegram, Slack, Feishu, LINE, WhatsApp Business, Discord, DingTalk, Mattermost, Matrix, SMS, BlueBubbles, Signal, WeCom, WeCom Callback, QQ Bot, Weixin, Microsoft Teams, Google Chat, and Gmail and run tasks in a paired workspace. Each paired chat has its own Kyler session. Home Assistant can submit prompts and receive replies as events.

## Enable channels

During installation or when running `kyler configure`, open **Channels** to
configure one or more platforms. Choose **Skip for now** on first setup or
**Keep current channels** when you already have channel settings. The wizard
masks credential input and saves the settings in the private configuration
file. Restart the backend to apply saved changes.

The platform tables below list the credentials and settings each adapter needs.
You can also set the same keys in the **Kyler server process environment**.
Environment values take precedence over saved values. Set
`KYLER_CHANNEL_WEB_URL` to the Web origin that users of the paired chats can reach.

Open **Channels** in Kyler Web to browse all platforms, including channels that
still need to be configured. Select a platform to view its setup requirements.
An enabled channel can generate a pairing code for a workspace and manage its
connected chats. Credentials are configured on the server.

### Telegram

Create a bot with BotFather.

| Variable | Value |
| --- | --- |
| `KYLER_TELEGRAM_BOT_TOKEN` | The bot token from BotFather |

Use one polling process per bot and disable any webhook already configured for it. The server needs outbound HTTPS access to `api.telegram.org`. See the [Telegram Bot API](https://core.telegram.org/bots/api#getupdates).

### Slack

Create and install a Slack app in one workspace. Enable the writable **Messages** tab in App Home. Enable bot events via the Events API request URL `/webhooks/channels/slack`. Subscribe to `message.im` and grant `im:history` and `chat:write`. Use the app's signing secret and the bot token issued for that workspace.

| Variable | Value |
| --- | --- |
| `KYLER_SLACK_BOT_TOKEN` | Bot user OAuth token |
| `KYLER_SLACK_SIGNING_SECRET` | App signing secret |
| `KYLER_SLACK_TEAM_ID` | Workspace ID |
| `KYLER_SLACK_APP_ID` | App ID |

See the [Slack Events API](https://docs.slack.dev/apis/events-api/) and [request signing](https://docs.slack.dev/authentication/verifying-requests-from-slack/).

### Feishu

Create a Feishu custom app, enable the bot, and subscribe to `im.message.receive_v1`. Give the bot permission to receive private messages and send messages as the app. Use an HTTPS event request URL with path `/webhooks/channels/feishu`. Leave Encrypt Key unset for this adapter, which checks the Verification Token in the HTTPS event body.

| Variable | Value |
| --- | --- |
| `KYLER_FEISHU_APP_ID` | Custom app ID |
| `KYLER_FEISHU_APP_SECRET` | App secret |
| `KYLER_FEISHU_VERIFICATION_TOKEN` | Event verification token |
| `KYLER_FEISHU_TENANT_KEY` | Expected tenant key |

See [Feishu event delivery](https://open.feishu.cn/document/event-subscription-guide/callback-subscription/receive-and-handle-callbacks).

### LINE

Create a LINE Official Account and enable its Messaging API channel. Set the webhook URL path to `/webhooks/channels/line` and enable webhooks. The adapter checks the signature and destination, then sends results using the push message API.

| Variable | Value |
| --- | --- |
| `KYLER_LINE_ACCESS_TOKEN` | Channel access token |
| `KYLER_LINE_CHANNEL_SECRET` | Channel secret |
| `KYLER_LINE_BOT_USER_ID` | Bot user ID (the webhook `destination`) |

See [LINE webhook configuration](https://developers.line.biz/en/docs/messaging-api/receiving-messages/) and [signature verification](https://developers.line.biz/en/docs/messaging-api/verify-webhook-signature/).

### WhatsApp Business

Configure a WhatsApp Cloud API business phone number and subscribe to the `messages` webhooks. Set the callback URL path to `/webhooks/channels/whatsapp`. This path handles Meta's verification challenge and signed incoming events.

| Variable | Value |
| --- | --- |
| `KYLER_WHATSAPP_ACCESS_TOKEN` | Cloud API access token with WhatsApp messaging permissions |
| `KYLER_WHATSAPP_APP_SECRET` | Meta app secret for event signatures |
| `KYLER_WHATSAPP_VERIFY_TOKEN` | Secret configured in Meta for the webhook verification challenge |
| `KYLER_WHATSAPP_PHONE_NUMBER_ID` | Cloud API phone number ID |
| `KYLER_WHATSAPP_GRAPH_VERSION` | Supported Graph API version for the app, in `vMAJOR.MINOR` format |

Before enabling this adapter, check the [WhatsApp Business Solution Terms](https://www.whatsapp.com/legal/business-solution-terms/) for restrictions on deploying general-purpose AI assistants.

Kyler sends free-form text replies in the customer service window. Configure WhatsApp templates separately if your deployment needs outbound messages outside that window. See the [WhatsApp Business Platform documentation](https://developers.facebook.com/documentation/business-messaging/whatsapp/overview/).

For Slack, Feishu, LINE, and WhatsApp, expose the corresponding webhook path on a public HTTPS reverse proxy pointing to the Kyler backend. Keep the gateway management API behind its normal access controls.

## Pair a chat

1. Open **Channels** from the Web sidebar. Channel management requires an owner gateway credential on the Web backend.
2. Select the platform and an authorized workspace and generate a pairing code.
3. Send the displayed pairing command in the bot's private chat within five minutes.
4. Send a text message. Use `!new` to begin another session or `!cancel` to request cancellation of the active run. Telegram also accepts `/new` and `/cancel`.

Open the Web session link to approve a pending tool request or answer a question. Run results are sent to the paired platform, so pair only chats that may receive the workspace's data.

**Disconnect** in Channels revokes the binding and removes queued messages and replies. Work that was already admitted can still finish. Cancel the run before disconnecting if it should stop.

Channel jobs and replies survive server restarts. Reply delivery is retried independently of model execution. In the event of an ambiguous delivery, the platform may show a duplicate reply. This release supports private text messages.

## Discord

Set the Interactions Endpoint URL to `/webhooks/channels/discord` on the
Kyler backend. Configure `KYLER_DISCORD_APPLICATION_ID`,
`KYLER_DISCORD_PUBLIC_KEY` and `KYLER_DISCORD_BOT_TOKEN` on the server.
Install the app with a bot and the `applications.commands` scope and
register the global command below through Discord's application command API.

```json
{
  "name": "kyler",
  "description": "Send a private message to Kyler",
  "type": 1,
  "contexts": [1],
  "options": [
    {"type": 3, "name": "text", "description": "Message or pairing command", "required": true}
  ]
}
```

In the bot's DM, invoke `/kyler` and put your message or the displayed
pairing command into the `text` option. Kyler verifies the interaction
signature and delivers the result as a separate bot message.
See [Discord application commands](https://docs.discord.com/developers/docs/interactions/slash-commands).

## DingTalk

Enable **Stream** delivery for a DingTalk internal enterprise robot.
Grant the robot permission to send one-to-one messages and make it
available to the intended users. This adapter uses an outbound WebSocket
connection and the DingTalk robot send API.

| Variable | Value |
| --- | --- |
| `KYLER_DINGTALK_APP_KEY` | Application client ID |
| `KYLER_DINGTALK_APP_SECRET` | Application client secret |
| `KYLER_DINGTALK_CORP_ID` | Allowed enterprise Corp ID |
| `KYLER_DINGTALK_ROBOT_CODE` | Robot code configured for the application |

In **Channels**, select **DingTalk** and pair through a private bot chat.
Existing external DingTalk bridge bindings belong to a separate namespace
and require pairing again. See the [official DingTalk Stream SDK](https://github.com/open-dingtalk/dingtalk-stream-sdk-go).

## Mattermost

Create a Mattermost bot account, issue a bot access token, and add the bot
to your Mattermost instance. Set the following server environment values:

| Variable | Value |
| --- | --- |
| `KYLER_MATTERMOST_URL` | HTTPS base URL of the instance, including its deployment subpath if present |
| `KYLER_MATTERMOST_BOT_TOKEN` | Bot access token |
| `KYLER_MATTERMOST_BOT_USER_ID` | User ID of that bot account |

Start a one-to-one DM with the bot and pair it from **Channels → Mattermost**.
Kyler listens on the Mattermost WebSocket and posts replies over REST.
This adapter processes live top-level text posts. It does not backfill
posts sent during downtime. Existing Mattermost bridge bindings use a
separate namespace and require pairing again.
See the [Mattermost API](https://docs.mattermost.com/api/).

## Matrix

Create a dedicated Matrix account for the bot and set these environment values:

| Variable | Value |
| --- | --- |
| `KYLER_MATRIX_URL` | HTTPS base URL of the homeserver |
| `KYLER_MATRIX_BOT_USER_ID` | Bot account Matrix user ID |
| `KYLER_MATRIX_ACCESS_TOKEN` | Access token for the bot account |

Join the bot and one user to an unencrypted, invite-only room. Set history
visibility to **joined members only**. The adapter checks the room and its
two participants before accepting messages or sending replies. Pair from
**Channels → Matrix** and send the displayed command in that room.

Kyler starts from a fresh sync whenever the server process starts. It
does not process historical messages during that initial sync or join
rooms automatically. Existing Matrix bridge bindings require pairing again.
See the [Matrix Client-Server API](https://spec.matrix.org/latest/client-server-api/).

## SMS (Twilio)

Configure a Twilio SMS-capable number to POST inbound messages to your public HTTPS callback. Use the exact callback URL in both Twilio and Kyler so request signatures match.

| Variable | Value |
| --- | --- |
| `KYLER_SMS_TWILIO_ACCOUNT_SID` | Twilio account SID |
| `KYLER_SMS_TWILIO_AUTH_TOKEN` | Twilio Auth Token |
| `KYLER_SMS_TWILIO_NUMBER` | Twilio number in E.164 format |
| `KYLER_SMS_TWILIO_WEBHOOK_URL` | Full public URL ending in `/webhooks/channels/sms` |
| `KYLER_SMS_ALLOWED_FROM` | Comma-separated list of permitted E.164 sender numbers |

Generate a pairing code in **Channels → SMS (Twilio)** and send it from an allowed number. Kyler acknowledges the webhook after queueing the message and sends replies through the Twilio Messages API. Use a workspace appropriate for SMS delivery. Existing SMS bridge bindings require pairing again. See [Twilio webhook security](https://www.twilio.com/docs/usage/webhooks/webhooks-security).

## Home Assistant

Configure the server with these values:

| Variable | Value |
| --- | --- |
| `KYLER_HOME_ASSISTANT_URL` | HTTPS base URL of the Home Assistant instance |
| `KYLER_HOME_ASSISTANT_TOKEN` | Long-lived access token with permission to subscribe to and fire events |
| `KYLER_HOME_ASSISTANT_USER_ID` | Authorized Home Assistant user ID |

Using the authenticated account named above, fire a `kyler_prompt` event with data `{"text":"!pair <code>"}` to pair. Submit later prompts the same way. Kyler publishes `kyler_reply` events with `id`, `user_id`, and `text`. Use an event-triggered automation to route those responses within Home Assistant. Authorized event-bus subscribers can see replies, so use an instance and workspace with the appropriate trust boundary. Existing bridge bindings require pairing again. See the [Home Assistant WebSocket API](https://developers.home-assistant.io/docs/api/websocket/) and [REST API](https://developers.home-assistant.io/docs/api/rest/).

## BlueBubbles

Connect Kyler to a BlueBubbles server on macOS. Set `KYLER_BLUEBUBBLES_URL`
to its HTTPS base URL and `KYLER_BLUEBUBBLES_PASSWORD` to the server
password. Set `KYLER_BLUEBUBBLES_CHATS` to a comma-separated allowlist of
direct chat GUIDs beginning with `iMessage;-;`.

Select **BlueBubbles** in Channels and send the pairing code from an
allowlisted iMessage chat. Kyler polls for new messages after startup and
sends plaintext replies through the server's AppleScript message API.
Existing bridge bindings require pairing again.
See the [BlueBubbles REST API](https://docs.bluebubbles.app/server/developer-guides/rest-api-and-webhooks).

## Signal

Run a registered or linked signal-cli account in daemon mode with a Unix
socket and manual receive mode. Keep the socket accessible only to trusted
local processes. Set `KYLER_SIGNAL_SOCKET` to its absolute path,
`KYLER_SIGNAL_ACCOUNT` to the account number or ACI, and
`KYLER_SIGNAL_ALLOWED_ACIS` to a comma-separated list of permitted sender
ACIs (UUIDs).

Pair in **Channels → Signal**. Kyler subscribes to private text messages
and sends replies through signal-cli JSON-RPC. Existing bridge bindings
require pairing again. See the [signal-cli JSON-RPC documentation](https://github.com/AsamK/signal-cli/blob/master/man/signal-cli-jsonrpc.5.adoc).

## WeCom

Create a WeCom intelligent bot with WebSocket delivery and configure
`KYLER_WECOM_BOT_ID` and `KYLER_WECOM_BOT_SECRET` on the Kyler server.
Kyler connects to the public WeCom WebSocket gateway and accepts private
text sent to the configured bot. It sends Markdown responses through the
same connection and waits for WeCom's acknowledgement.

Select **WeCom** in Channels and pair from the bot's private chat.
Previous bridge bindings require pairing again.
See the [official WeCom intelligent bot SDK](https://github.com/WecomTeam/aibot-node-sdk).

## WeCom Callback

Create an internal WeCom application and enable message callbacks. Use
the public HTTPS callback path `/webhooks/channels/wecom_callback`.

| Variable | Value |
| --- | --- |
| `KYLER_WECOM_CALLBACK_CORP_ID` | Enterprise Corp ID |
| `KYLER_WECOM_CALLBACK_AGENT_ID` | Internal application agent ID |
| `KYLER_WECOM_CALLBACK_APP_SECRET` | Internal application secret |
| `KYLER_WECOM_CALLBACK_TOKEN` | Callback verification token |
| `KYLER_WECOM_CALLBACK_AES_KEY` | The 43-character EncodingAESKey |

Kyler handles the encrypted URL verification challenge and accepts signed
text messages for the configured application. Replies use the WeCom
application message API. Select **WeCom Callback** for pairing. The
separate **WeCom** intelligent bot uses its own credentials.
See [WeCom callback configuration](https://developer.work.weixin.qq.com/document/path/90930).

## QQ Bot

Create a QQ bot and configure **Channels → QQ Bot** with `kyler configure`.
Enter the App ID and App Secret, then choose a delivery mode. New setup recommends
**WebSocket**; existing configurations without a mode keep using **Webhook**.
Save and restart the backend to apply the setting. Credentials remain on the server.

| Variable | Value |
| --- | --- |
| `KYLER_QQBOT_APP_ID` | QQ bot App ID |
| `KYLER_QQBOT_APP_SECRET` | QQ bot App Secret |
| `KYLER_QQBOT_MODE` | `websocket` or `webhook`; omitted preserves legacy `webhook` behavior |

### WebSocket (local or server deployments)

Select **WebSocket** in the QQ developer console under **Development settings →
Event subscriptions and callbacks**, and select the same mode in Kyler's wizard.
Kyler opens an outbound connection to the official gateway, authenticates, maintains
heartbeats, and reconnects with exponential backoff. No public inbound port, domain,
or HTTPS callback is required. Allow outbound HTTPS/WSS access to the QQ APIs and
gateway. Use only one running Kyler listener for a bot.

The adapter subscribes to `GROUP_AND_C2C_EVENT` (`1 << 25`) and processes only
`C2C_MESSAGE_CREATE` private text messages. Grant the required permissions in the
QQ console; group events are ignored. Transient reconnects resume the session from
the last durably processed event, and replayed messages are deduplicated. A backend
restart creates a fresh gateway session; existing paired chats remain stored.
Permission, shard, and bot-status rejections stop the listener with an actionable
server log; fix the console configuration and restart. If your bot requires an
OpenAPI IP allowlist, authorize the deployment's public outbound IP.

### Webhook (public HTTPS deployments)

Choose **Webhook** in both Kyler's wizard and the QQ console. Configure the public
HTTPS callback path `/webhooks/channels/qqbot`, proxy it to the Kyler backend, and
subscribe to `C2C_MESSAGE_CREATE`. Kyler verifies callback signatures and completes
URL verification. The webhook route does not accept events in WebSocket mode.

### Pairing and replies

Select **QQ Bot** in Kyler Web's Channels and generate a workspace pairing code.
Send `!pair <code>` from a private bot conversation, then send a text prompt.
Both delivery modes share the same stored bindings, sessions, deduplication, and
reply flow, so switching modes does not require pairing again.

Replies retain the original QQ message ID and a stable sequence number. Reply
delivery uses the one-hour private reply window, with a conservative cap of four
replies per incoming message. Open the session in Kyler Web for results that
exceed those limits.
QQ replies are rendered as plain text: headings and emphasis lose their Markdown
markers, code keeps its content and line breaks, links retain public URLs, and
table rows become readable lines. Original Markdown remains in the Web session.

Private QQ messages can include images or files, with or without text. Both
WebSocket and Webhook delivery preserve attachments in the durable job queue.
Kyler downloads attachments over HTTPS from QQ CDN hosts into the session's
private attachment store; it does not place them in arbitrary workspace paths.
Each message supports up to eight attachments, each up to 20 MiB. Supported input
formats follow Kyler's attachment reader: images, PDF, DOCX, XLSX, CSV/TSV and
UTF-8 text. A vision-capable model is required for images; documents require a
model with tool support. Unsupported or expired attachments produce a message
instead of silently starting a task without the files.

For outgoing files, the agent must explicitly register its completed deliverables
with `register_artifacts`. After the run completes, Kyler queues immutable artifacts
from that execution only. Text goes first, followed by up to three files (fewer if
an earlier notice used a reply slot). Files beyond the available reply budget
remain downloadable in Kyler Web. PNG/JPEG artifacts are sent as images; other
artifacts use QQ file messages, subject to the bot's platform permissions.
Small files use binary upload; files over 8 MiB use QQ's presigned multipart
upload, up to Kyler's 20 MiB artifact limit. No public file server is required.

Attachment IDs and outgoing file jobs survive process restarts. Cached media
handles and stable reply sequence numbers are reused for retries. File transfers
have bounded retries; a permanent failure becomes a plain-text notification with
a Web session link while successful text/file queue entries stay completed.
A network timeout can still leave delivery uncertain at the platform boundary;
this is not a guarantee of exactly-once delivery. Expired reply windows cannot
be extended by retrying; open the session in Web to retrieve the files.

Previous bridge bindings require pairing again.
See [QQ event subscriptions and Gateway lifecycle](https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/interface-framework/event-emit.html),
the [QQ Bot SDK](https://github.com/tencent-connect/botgo), and the
[reply API](https://github.com/tencent-connect/bot-docs/blob/main/docs/develop/api-v2/server-inter/message/send-receive/send.md).

## Weixin (iLink)

Authorize an iLink bot account using Tencent's QR login flow and configure
the resulting credentials in the Kyler server environment:

| Variable | Value |
| --- | --- |
| `KYLER_WEIXIN_BOT_TOKEN` | Authorized iLink bot token |
| `KYLER_WEIXIN_BOT_ID` | iLink bot ID returned by login |
| `KYLER_WEIXIN_USER_ID` | Authorized Weixin user ID returned by login |
| `KYLER_WEIXIN_URL` | Official HTTPS base URL returned by login, if different from the default |

Use one polling process per bot account. After Kyler starts, select
**Weixin** in Channels and send the pairing command in the authorized
private chat. Kyler stores the polling cursor and message-specific
reply context in its private channel database. This adapter uses
existing iLink credentials for text conversations. QR login takes
place in Tencent's authorization client. Prior bridge bindings require
pairing again.
See [Tencent's iLink protocol and QR authorization flow](https://github.com/Tencent/openclaw-weixin/blob/main/docs/protocol.md).

### Microsoft Teams

Register a single-tenant Azure Bot, enable the Microsoft Teams channel,
and set its public HTTPS messaging endpoint to `/webhooks/channels/teams`.
Install the Teams app with the bot's personal scope. Kyler accepts text
from personal chats in the configured tenant and sends replies via the
public Bot Connector. Open Channels in Kyler Web and send the pairing
command in the bot's personal chat.

| Variable | Value |
| --- | --- |
| `KYLER_TEAMS_APP_ID` | Microsoft Entra application (client) ID |
| `KYLER_TEAMS_TENANT_ID` | Microsoft Entra directory (tenant) ID |
| `KYLER_TEAMS_CLIENT_SECRET` | Bot application client secret |

See [Bot Connector authentication](https://learn.microsoft.com/en-us/azure/bot-service/rest-api/bot-framework-rest-connector-authentication).

### Google Chat

Enable the Google Chat API in the app's Cloud project and configure a Chat
app that supports direct messages. In the Chat API configuration, set the
HTTP endpoint to `/webhooks/channels/google_chat` on your HTTPS origin and
set the authentication audience to **HTTP endpoint URL**. Use a service
account from the same project for replies. Pair from a direct message
with the app.

| Variable | Value |
| --- | --- |
| `KYLER_GOOGLE_CHAT_WEBHOOK_URL` | Exact public HTTPS endpoint registered in Google Chat |
| `KYLER_GOOGLE_CHAT_CREDENTIALS_FILE` | Server-readable path to the service account key JSON |

See [verify requests from Chat](https://developers.google.com/workspace/chat/verify-requests-from-chat) and [authenticate as a Chat app](https://developers.google.com/workspace/chat/authenticate-authorize-chat-app).

### Email (Gmail)

Enable the Gmail API and obtain an OAuth refresh token for a dedicated
mailbox with the `https://www.googleapis.com/auth/gmail.modify` scope.
Kyler polls the inbox and sends replies to a single configured address.

| Variable | Value |
| --- | --- |
| `KYLER_EMAIL_ADDRESS` | Gmail mailbox authorized by the refresh token |
| `KYLER_EMAIL_ALLOWED_FROM` | Address that can submit commands and receive replies |
| `KYLER_EMAIL_MESSAGE_SECRET` | Random secret of at least 32 non-space ASCII characters |
| `KYLER_EMAIL_GOOGLE_CLIENT_ID` | OAuth client ID |
| `KYLER_EMAIL_GOOGLE_CLIENT_SECRET` | OAuth client secret |
| `KYLER_EMAIL_GOOGLE_REFRESH_TOKEN` | Gmail mailbox refresh token |

Send a fresh plain-text email with subject `Kyler`. On the first line,
write `Kyler-Auth: <message secret>`. On the following lines, write the
pairing command from Kyler Web or a prompt. Kyler strips the authentication
line before ingesting the text and marks accepted messages read after
they have been stored. Use one polling process per mailbox.

See [Gmail API scopes](https://developers.google.com/workspace/gmail/api/auth/scopes).

If Teams, Google Chat, or Email was previously paired through a Kyler
bridge profile, pair again using its platform adapter.

## API Server and Webhook

These two channels use Kyler's HTTP protocol. Configure
`KYLER_CHANNEL_<PROFILE>_INSTANCE` and `KYLER_CHANNEL_<PROFILE>_SECRET`,
where `<PROFILE>` is `API_SERVER` or `WEBHOOK`. Use a stable instance ID
and a random secret of at least 32 bytes. All configured channels also
require `KYLER_CHANNEL_WEB_URL`.

POST to `/webhooks/channels/<profile>` with this JSON body:

```json
{"id":"event_001","chat":"chat_001","text":"hello","private":true}
```

The lowercase profile in the URL is `api_server` or `webhook`. Use
stable IDs for the chat and the original event, and reuse the event ID
when retrying. IDs may contain ASCII letters, digits, underscores and
hyphens, must contain at least one letter and are limited to 128 characters.

Set `X-Kyler-Timestamp` to current Unix seconds. Compute HMAC-SHA256 with
the profile secret over the UTF-8 prefix
`v1:event:<profile>:<timestamp>:` followed by the exact transmitted body
bytes. Send the digest as `X-Kyler-Signature: sha256=<hex>`. The timestamp
must be within five minutes. A retry may sign again with a new timestamp
while retaining the original event ID.

For `api_server`, read queued replies using `GET /channel-api/replies` with
`Authorization: Bearer <profile secret>`. HTTP 204 means the queue is
empty. Each reply includes `id`, `chat`, and `text`. Acknowledge it with
`DELETE /channel-api/replies/<id>` using the same authentication.
Use a single consumer and deduplicate by reply ID if a response is handled
before the acknowledgement succeeds.

For `webhook`, also configure `KYLER_CHANNEL_WEBHOOK_REPLY_URL` (HTTPS)
and `KYLER_CHANNEL_WEBHOOK_REPLY_SECRET` (a different random secret of at
least 32 bytes). Kyler sends `{"channel","id","chat","text"}` to that URL.
The reply uses the same signature headers with the separate reply secret
and prefix `v1:reply:<profile>:<timestamp>:`. Deduplicate by reply ID before
returning a success response.
