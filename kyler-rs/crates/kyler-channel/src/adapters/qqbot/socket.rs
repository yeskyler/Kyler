//! QQ Gateway protocol. Session checkpoints advance only after durable ingestion.
use super::{QqBot, incoming};
use crate::{ChannelStore, StreamStopped};

use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::{Instant, interval_at, timeout},
};
use tokio_tungstenite::{
    WebSocketStream, connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

#[derive(Default)]
pub(super) struct Session {
    id: Option<String>,
    sequence: Option<u64>,
    next_identify: Option<Instant>,
    next_connect: Option<Instant>,
    invalid_sessions: u8,
}
impl Session {
    fn reset(&mut self) {
        self.id = None;
        self.sequence = None;
    }
    fn authenticate(&self, token: &str) -> Value {
        match (&self.id, self.sequence) {
            (Some(id), Some(sequence)) => json!({"op":6,"d":{
                "token":format!("QQBot {token}"),"session_id":id,"seq":sequence}}),
            _ => json!({"op":2,"d":{
                "token":format!("QQBot {token}"),"intents":1u32<<25,"shard":[0,1]}}),
        }
    }
}

impl QqBot {
    pub(crate) async fn listen(&self, store: &ChannelStore) -> Result<()> {
        ensure!(self.websocket, "QQ Bot is configured for Webhook delivery");
        let mut session = self.session.lock().await;
        if let Some(next) = session.next_connect.take() {
            tokio::time::sleep_until(next).await;
        }
        let token = self.token().await.map_err(|status| {
            if matches!(status, Some(401 | 403)) {
                anyhow::Error::new(StreamStopped(
                    "QQ Bot credentials were rejected; check App ID and App Secret, then restart",
                ))
            } else {
                anyhow::anyhow!("QQ Bot access token request failed")
            }
        })?;
        let response = self
            .http
            .get("https://api.bot.qq.com/gateway")
            .header("authorization", format!("QQBot {token}"))
            .header("x-union-appid", &self.app)
            .send()
            .await
            .context("QQ Bot gateway request failed")?;
        if response.status().as_u16() == 401 {
            *self.access.lock().await = None;
        }
        ensure!(
            response.status().is_success(),
            "QQ Bot gateway request was rejected (HTTP {})",
            response.status().as_u16()
        );
        let data: Value = response
            .json()
            .await
            .context("Invalid QQ Bot gateway response")?;
        let url = data["url"].as_str().context("Missing QQ Bot gateway URL")?;
        ensure!(valid_gateway(url), "Invalid QQ Bot gateway URL");
        let config = WebSocketConfig::default()
            .max_message_size(Some(256 * 1024))
            .max_frame_size(Some(256 * 1024));
        let (mut socket, _) = timeout(
            Duration::from_secs(15),
            connect_async_with_config(url, Some(config), false),
        )
        .await
        .context("QQ Bot gateway connection timed out")?
        .map_err(|_| anyhow::anyhow!("QQ Bot gateway connection failed"))?;
        self.run_socket(&mut socket, store, &token, &mut session)
            .await
    }

    async fn run_socket<S>(
        &self,
        socket: &mut WebSocketStream<S>,
        store: &ChannelStore,
        token: &str,
        session: &mut Session,
    ) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let result = self.exchange(socket, store, token, session).await;
        if let Err(error) = &result {
            if let Some(close) = error.downcast_ref::<GatewayClose>() {
                if matches!(close.0, 4006 | 4007 | 4900..=4913) {
                    session.reset();
                }
                if close.0 == 4004 {
                    *self.access.lock().await = None;
                }
                if close.0 == 4008 {
                    session.next_connect = Some(Instant::now() + Duration::from_secs(60));
                }
                if let Some(reason) = terminal_close(close.0) {
                    return Err(StreamStopped(reason).into());
                }
            }
        }
        result
    }

    async fn exchange<S>(
        &self,
        socket: &mut WebSocketStream<S>,
        store: &ChannelStore,
        token: &str,
        session: &mut Session,
    ) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let hello = timeout(Duration::from_secs(15), read_payload(socket))
            .await
            .context("QQ Bot gateway did not send Hello")??;
        ensure!(hello["op"] == 10, "QQ Bot gateway expected Hello");
        let period = hello["d"]["heartbeat_interval"]
            .as_u64()
            .filter(|ms| (100..=300_000).contains(ms))
            .context("Invalid QQ Bot heartbeat interval")?;
        if session.id.is_none() {
            if let Some(next) = session.next_identify {
                tokio::time::sleep_until(next).await;
            }
            session.next_identify = Some(Instant::now() + Duration::from_secs(5));
        }
        send(socket, session.authenticate(token)).await?;
        let mut timer = interval_at(
            Instant::now() + Duration::from_millis(period),
            Duration::from_millis(period),
        );
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let deadline = tokio::time::sleep(Duration::from_secs(15));
        tokio::pin!(deadline);
        let mut authenticated = false;
        let mut awaiting_ack = false;
        loop {
            tokio::select! {
                _ = &mut deadline, if !authenticated => bail!("QQ Bot gateway authentication timed out"),
                _ = timer.tick() => {
                    ensure!(!awaiting_ack, "QQ Bot missed its heartbeat acknowledgement");
                    send(socket, json!({"op":1,"d":session.sequence})).await?;
                    awaiting_ack = true;
                }
                payload = read_payload(socket) => {
                    let payload = payload?;
                    match payload["op"].as_u64() {
                        Some(0) => {
                            let sequence = payload["s"].as_u64().context("Missing QQ Bot event sequence")?;
                            match payload["t"].as_str() {
                                Some("READY") => {
                                    let id = payload["d"]["session_id"].as_str().filter(|s| !s.is_empty() && s.len() <= 512)
                                        .context("Missing QQ Bot session ID")?;
                                    session.id = Some(id.into());
                                    authenticated = true;
                                    session.invalid_sessions = 0;
                                }
                                Some("RESUMED") => { authenticated = true; session.invalid_sessions = 0; }
                                _ => {
                                    ensure!(session.id.is_some(), "QQ Bot event arrived before authentication");
                                    if let Some(message) = incoming(&payload, chrono::Utc::now().timestamp()) {
                                        store.ingest(vec![message]).await.context("QQ Bot message persistence failed")?;
                                    }
                                }
                            }
                            session.sequence = Some(sequence);
                        }
                        Some(1) => {
                            send(socket, json!({"op":1,"d":session.sequence})).await?;
                            awaiting_ack = true;
                        }
                        Some(7) => bail!("QQ Bot gateway requested reconnection"),
                        Some(9) => {
                            session.reset();
                            session.invalid_sessions = session.invalid_sessions.saturating_add(1);
                            if session.invalid_sessions >= 3 {
                                return Err(StreamStopped("QQ Bot repeatedly rejected authentication; check credentials and C2C permissions, then restart").into());
                            }
                            bail!("QQ Bot session is invalid; reconnecting with fresh authentication");
                        }
                        Some(11) => awaiting_ack = false,
                        _ => {}
                    }
                }
            }
        }
    }
}

fn valid_gateway(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "wss"
            && url.username().is_empty()
            && url.password().is_none()
            && url.host_str().is_some_and(|host| host.ends_with(".qq.com"))
    })
}

async fn send<S>(socket: &mut WebSocketStream<S>, payload: Value) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    timeout(
        Duration::from_secs(10),
        socket.send(Message::Text(payload.to_string().into())),
    )
    .await
    .context("QQ Bot gateway send timed out")?
    .map_err(|_| anyhow::anyhow!("QQ Bot gateway send failed"))
}

#[derive(Debug)]
struct GatewayClose(u16);
impl std::fmt::Display for GatewayClose {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "QQ Bot gateway closed (code {})", self.0)
    }
}
impl std::error::Error for GatewayClose {}

async fn read_payload<S>(socket: &mut WebSocketStream<S>) -> Result<Value>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    while let Some(message) = socket.next().await {
        match message.map_err(|_| anyhow::anyhow!("QQ Bot gateway receive failed"))? {
            Message::Text(text) => {
                return serde_json::from_str(&text).context("Invalid QQ Bot gateway payload");
            }
            Message::Close(frame) => {
                return Err(GatewayClose(frame.map_or(1000, |f| f.code.into())).into());
            }
            Message::Ping(_) => {
                timeout(Duration::from_secs(10), socket.flush())
                    .await
                    .context("QQ Bot gateway pong timed out")?
                    .map_err(|_| anyhow::anyhow!("QQ Bot gateway pong failed"))?;
            }
            _ => {}
        }
    }
    bail!("QQ Bot gateway disconnected")
}

fn terminal_close(code: u16) -> Option<&'static str> {
    match code {
        4001 | 4002 => {
            Some("QQ Bot gateway rejected the protocol payload; check the installed Kyler version")
        }
        4010 | 4011 => {
            Some("QQ Bot shard configuration was rejected; check bot gateway requirements")
        }
        4012 => Some("QQ Bot gateway version is unsupported"),
        4013 | 4014 => Some(
            "QQ Bot C2C event subscription was rejected; enable group and C2C message permissions, then restart",
        ),
        4914 => Some("QQ Bot is restricted to sandbox use; check its release status, then restart"),
        4915 => Some("QQ Bot is disabled; check its status in the QQ console, then restart"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::DuplexStream;
    use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, frame::coding::CloseCode};

    fn test_bot() -> QqBot {
        QqBot {
            app: "123456".into(),
            websocket: true,
            secret: "TEST_SECRET".into(),
            signing: super::super::key("TEST_SECRET").unwrap(),
            http: reqwest::Client::builder()
                .no_proxy()
                .tls_certs_only([])
                .build()
                .unwrap(),
            access: tokio::sync::Mutex::new(None),
            session: tokio::sync::Mutex::new(Session::default()),
        }
    }
    async fn sockets() -> (WebSocketStream<DuplexStream>, WebSocketStream<DuplexStream>) {
        let (client, server) = tokio::io::duplex(256 * 1024);
        tokio::join!(
            WebSocketStream::from_raw_socket(client, Role::Client, None),
            WebSocketStream::from_raw_socket(server, Role::Server, None)
        )
    }
    fn event(sequence: u64, id: &str, text: &str) -> Value {
        json!({"op":0,"s":sequence,"t":"C2C_MESSAGE_CREATE","d":{
            "id":id,"author":{"user_openid":"user"},"content":text,
            "timestamp":chrono::Utc::now().to_rfc3339()}})
    }
    async fn hello(socket: &mut WebSocketStream<DuplexStream>, period: u64) -> Result<Value> {
        send(socket, json!({"op":10,"d":{"heartbeat_interval":period}})).await?;
        read_payload(socket).await
    }
    async fn ready(socket: &mut WebSocketStream<DuplexStream>) -> Result<()> {
        send(
            socket,
            json!({"op":0,"s":1,"t":"READY","d":{"session_id":"test-session"}}),
        )
        .await
    }

    #[tokio::test]
    async fn private_events_pair_deduplicate_and_resume_from_durable_sequence() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = ChannelStore::open(&root.path().join("channel.sqlite"), "qqbot:123456".into())?;
        let code = store.pair("credential".into(), "/workspace".into()).await?;
        let bot = test_bot();
        let mut session = Session::default();
        let (mut socket, mut server) = sockets().await;
        let peer = async {
            let auth = hello(&mut server, 10_000).await?;
            assert_eq!(auth["op"], 2);
            assert_eq!(auth["d"]["intents"], 1u32 << 25);
            assert_eq!(auth["d"]["shard"], json!([0, 1]));
            ready(&mut server).await?;
            send(&mut server, event(2, "pair", &format!("!pair {code}"))).await?;
            send(&mut server, event(3, "message", "hello")).await?;
            send(&mut server, event(4, "message", "hello")).await?;
            send(&mut server, json!({"op":7})).await
        };
        let (result, peer) = timeout(Duration::from_secs(5), async {
            tokio::join!(
                bot.run_socket(&mut socket, &store, "TEST_TOKEN", &mut session),
                peer
            )
        })
        .await?;
        peer?;
        assert!(result.is_err());
        assert_eq!(session.sequence, Some(4));
        let jobs = store.jobs().await?;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].input, "hello");
        assert_eq!(
            store
                .next_reply()
                .await?
                .unwrap()
                .receipt
                .unwrap()
                .message_id,
            "qq_pair"
        );
        let (mut socket, mut server) = sockets().await;
        let peer = async {
            let auth = hello(&mut server, 10_000).await?;
            assert_eq!(auth["op"], 6);
            assert_eq!(auth["d"]["seq"], 4);
            assert_eq!(auth["d"]["session_id"], "test-session");
            send(&mut server, json!({"op":0,"s":5,"t":"RESUMED","d":null})).await?;
            send(&mut server, event(6, "message", "hello")).await?;
            send(&mut server, json!({"op":1})).await?;
            let heartbeat = read_payload(&mut server).await?;
            assert_eq!(heartbeat, json!({"op":1,"d":6}));
            send(&mut server, json!({"op":11})).await?;
            send(&mut server, json!({"op":7})).await
        };
        let (result, peer) = timeout(Duration::from_secs(5), async {
            tokio::join!(
                bot.run_socket(&mut socket, &store, "REFRESHED_TEST_TOKEN", &mut session),
                peer
            )
        })
        .await?;
        peer?;
        assert!(result.is_err());
        assert_eq!(session.sequence, Some(6));
        assert_eq!(store.jobs().await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn missing_heartbeat_ack_disconnects_instead_of_leaving_a_dead_socket() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = ChannelStore::open(&root.path().join("channel.sqlite"), "qqbot:123456".into())?;
        let bot = test_bot();
        let mut session = Session::default();
        let (mut socket, mut server) = sockets().await;
        let peer = async {
            hello(&mut server, 100).await?;
            ready(&mut server).await?;
            assert_eq!(read_payload(&mut server).await?, json!({"op":1,"d":1}));
            tokio::time::sleep(Duration::from_millis(350)).await;
            Ok::<_, anyhow::Error>(())
        };
        let (result, peer) = timeout(Duration::from_secs(5), async {
            tokio::join!(
                bot.run_socket(&mut socket, &store, "TEST_TOKEN", &mut session),
                peer
            )
        })
        .await?;
        peer?;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("heartbeat acknowledgement")
        );
        assert_eq!(session.id.as_deref(), Some("test-session"));
        Ok(())
    }

    #[tokio::test]
    async fn invalid_session_resets_authentication_and_terminal_close_is_actionable() -> Result<()>
    {
        let root = tempfile::tempdir()?;
        let store = ChannelStore::open(&root.path().join("channel.sqlite"), "qqbot:123456".into())?;
        let bot = test_bot();
        let mut session = Session {
            id: Some("old-session".into()),
            sequence: Some(99),
            ..Default::default()
        };
        let (mut socket, mut server) = sockets().await;
        let peer = async {
            assert_eq!(hello(&mut server, 10_000).await?["op"], 6);
            send(&mut server, json!({"op":9,"d":false})).await
        };
        let (result, peer) = tokio::join!(
            bot.run_socket(&mut socket, &store, "TEST_TOKEN", &mut session),
            peer
        );
        peer?;
        assert!(result.is_err());
        assert!(session.id.is_none());
        assert!(session.sequence.is_none());
        assert_eq!(session.authenticate("TEST_TOKEN")["op"], 2);
        let (mut socket, mut server) = sockets().await;
        server
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::Library(4014),
                reason: "PRIVATE_REMOTE_BODY".into(),
            })))
            .await?;
        let error = bot
            .run_socket(&mut socket, &store, "TEST_TOKEN", &mut session)
            .await
            .unwrap_err();
        assert!(error.downcast_ref::<StreamStopped>().is_some());
        assert!(error.to_string().contains("permissions"));
        assert!(!error.to_string().contains("PRIVATE_REMOTE_BODY"));
        Ok(())
    }

    #[test]
    fn gateway_validation_and_transport_selection_keep_existing_webhooks_compatible() {
        assert!(valid_gateway("wss://api.bot.qq.com/websocket/"));
        assert!(valid_gateway("wss://api.sgroup.qq.com/websocket/"));
        for url in [
            "ws://api.bot.qq.com/",
            "wss://qq.com.attacker.test/",
            "wss://user:password@api.bot.qq.com/",
            "wss://127.0.0.1/",
        ] {
            assert!(!valid_gateway(url));
        }
        let mut bot = test_bot();
        bot.websocket = false;
        let adapter = crate::Adapter::QqBot(bot);
        assert!(adapter.accepts_webhook());
        assert!(!adapter.streams());
        let mut bot = test_bot();
        bot.websocket = true;
        let adapter = crate::Adapter::QqBot(bot);
        assert!(!adapter.accepts_webhook());
        assert!(adapter.streams());
        assert_eq!(adapter.namespace(), "qqbot:123456");
    }
}
