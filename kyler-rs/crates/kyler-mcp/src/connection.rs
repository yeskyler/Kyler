//! Transport-independent protocol sessions, discovery and immutable catalogs.
use crate::{ToolCatalog, ToolDescriptor};

use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use anyhow::{Result, ensure};
use rmcp::{
    ClientHandler, ServiceExt,
    service::{NotificationContext, RoleClient, RunningService},
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use tokio::io::{AsyncRead, AsyncWrite};

/// Implemented by the host after applying its process/network policy.
#[async_trait::async_trait]
pub trait TransportFactory: Send + Sync {
    async fn connect(&self) -> Result<Connection>;
}
/// Keeps host-owned processes alive and stops them on disconnect or failure.
pub trait TransportGuard: Send + Sync {
    fn stop(&self);
}
struct GuardLease(Arc<dyn TransportGuard>);
impl Drop for GuardLease {
    fn drop(&mut self) {
        self.0.stop();
    }
}
#[derive(Clone)]
pub(crate) struct Handler(Arc<AtomicBool>);
impl ClientHandler for Handler {
    async fn on_tool_list_changed(&self, _: NotificationContext<RoleClient>) {
        self.0.store(true, Ordering::SeqCst);
    }
}
pub struct Connection {
    approval_id: String,
    pub(crate) service: RunningService<RoleClient, Handler>,
    guard: Option<GuardLease>,
    pub(crate) enabled: AtomicBool,
    pub(crate) calls: AtomicUsize,
    changed: Arc<AtomicBool>,
    catalog: RwLock<Arc<ToolCatalog>>,
}
impl Connection {
    pub async fn from_io<S: AsyncRead + AsyncWrite + Send + 'static>(
        stream: S,
        guard: Arc<dyn TransportGuard>,
    ) -> Result<Self> {
        let guard = GuardLease(guard);
        let changed = Arc::new(AtomicBool::new(false));
        let service = Handler(changed.clone()).serve(stream).await?;
        Self::finish(service, changed, Some(guard)).await
    }
    pub async fn from_http(
        client: reqwest::Client,
        url: &str,
        token: Option<String>,
    ) -> Result<Self> {
        let changed = Arc::new(AtomicBool::new(false));
        let mut options = StreamableHttpClientTransportConfig::with_uri(url.to_owned());
        if let Some(token) = token {
            options = options.auth_header(token);
        }
        // Never replay tools/call after an expired remote session.
        options = options.reinit_on_expired_session(false);
        let service = Handler(changed.clone())
            .serve(StreamableHttpClientTransport::with_client(client, options))
            .await?;
        Self::finish(service, changed, None).await
    }
    async fn finish(
        service: RunningService<RoleClient, Handler>,
        changed: Arc<AtomicBool>,
        guard: Option<GuardLease>,
    ) -> Result<Self> {
        let catalog = Arc::new(ToolCatalog::new(discover(&service).await?)?);
        Ok(Self {
            approval_id: uuid::Uuid::new_v4().to_string(),
            service,
            guard,
            enabled: AtomicBool::new(true),
            calls: AtomicUsize::new(0),
            changed,
            catalog: RwLock::new(catalog),
        })
    }
    pub fn approval_id(&self) -> &str {
        &self.approval_id
    }
    pub fn catalog(&self) -> Arc<ToolCatalog> {
        self.catalog.read().unwrap().clone()
    }
    pub fn is_available(&self) -> bool {
        self.enabled.load(Ordering::SeqCst) && !self.service.is_closed()
    }
    pub(crate) fn needs_refresh(&self) -> bool {
        self.changed.swap(false, Ordering::SeqCst)
    }
    pub(crate) async fn refresh(&self) -> Result<()> {
        let catalog = ToolCatalog::new(discover(&self.service).await?)?;
        *self.catalog.write().unwrap() = Arc::new(catalog);
        Ok(())
    }
    pub(crate) fn stop(&self) {
        self.enabled.store(false, Ordering::SeqCst);
        self.service.cancellation_token().cancel();
        if let Some(guard) = &self.guard {
            guard.0.stop();
        }
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.stop();
    }
}
async fn discover(service: &RunningService<RoleClient, Handler>) -> Result<Vec<ToolDescriptor>> {
    let mut tools = Vec::new();
    let mut cursor = None;
    let mut cursors = std::collections::HashSet::new();
    loop {
        let mut params = rmcp::model::PaginatedRequestParams::default();
        params.cursor = cursor;
        let page = service.list_tools(Some(params)).await?;
        ensure!(
            tools.len() + page.tools.len() <= 256,
            "MCP tool count exceeds 256"
        );
        tools.extend(
            page.tools
                .into_iter()
                .map(ToolDescriptor::from_remote)
                .collect::<Result<Vec<_>>>()?,
        );
        cursor = page.next_cursor;
        let Some(next) = &cursor else { break };
        ensure!(
            cursors.insert(next.clone()) && tools.len() < 256,
            "MCP pagination exceeds limit or repeats a cursor"
        );
    }
    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Client;
    use kyler_protocol::api::mcp::McpAction;
    use serde_json::{Value, json};
    use std::{sync::atomic::AtomicUsize, time::Duration};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    struct Guard(Arc<AtomicBool>);
    impl TransportGuard for Guard {
        fn stop(&self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    struct Factory {
        stopped: Arc<AtomicBool>,
        calls: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl TransportFactory for Factory {
        async fn connect(&self) -> Result<Connection> {
            let (client, server) = tokio::io::duplex(64 * 1024);
            let calls = self.calls.clone();
            tokio::spawn(async move {
                let (read, mut write) = tokio::io::split(server);
                let mut lines = BufReader::new(read).lines();
                let mut changed = false;
                while let Ok(Some(line)) = lines.next_line().await {
                    let message: Value = serde_json::from_str(&line).unwrap();
                    let Some(id) = message.get("id") else {
                        continue;
                    };
                    let method = message["method"].as_str().unwrap();
                    let result = match method {
                        "initialize" => {
                            json!({"protocolVersion":message["params"]["protocolVersion"],"capabilities":{"tools":{"listChanged":true}},"serverInfo":{"name":"fixture","version":"1"}})
                        }
                        "tools/list" => {
                            json!({"tools":[{"name":"echo","description":if changed {"Updated catalog"} else {"Original catalog"},"inputSchema":{"type":"object"}}]})
                        }
                        "tools/call" => {
                            calls.fetch_add(1, Ordering::SeqCst);
                            json!({"content":[{"type":"text","text":"fixture"}],"structuredContent":{"ok":true},"isError":message["params"]["arguments"]["fail"]==true})
                        }
                        _ => json!({}),
                    };
                    let response =
                        json!({"jsonrpc":"2.0","id":id,"result":result}).to_string() + "\n";
                    if write.write_all(response.as_bytes()).await.is_err() {
                        break;
                    }
                    if method == "tools/call" && !changed {
                        changed = true;
                        if write.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n").await.is_err(){break}
                    }
                }
            });
            Connection::from_io(client, Arc::new(Guard(self.stopped.clone()))).await
        }
    }
    #[tokio::test]
    async fn refresh_invalidates_old_catalog_calls_and_disconnect_stops_host_guard() {
        let stopped = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let (changes, _) = tokio::sync::watch::channel(0);
        let client = Client::new(
            "fixture".into(),
            "stdio".into(),
            Arc::new(Factory {
                stopped: stopped.clone(),
                calls: calls.clone(),
            }),
            Duration::from_secs(2),
            false,
            changes,
        );
        client.change(McpAction::Connect).await;
        let connection = client.connection().unwrap();
        let old = connection.catalog().version.clone();
        assert!(
            connection
                .call("echo", &old, r#"{"fail":true}"#, Duration::from_secs(2))
                .await
                .is_err()
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "error calls must not be retried"
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            while connection.catalog().version == old {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            connection
                .call("echo", &old, "{}", Duration::from_secs(2))
                .await
                .is_err()
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "stale schema must not reach the server"
        );
        let current = connection.catalog().version.clone();
        assert!(
            connection
                .call("echo", &current, "{}", Duration::from_secs(2))
                .await
                .unwrap()
                .contains("structuredContent")
        );
        client.change(McpAction::Disconnect).await;
        assert!(stopped.load(Ordering::SeqCst));
        assert!(
            connection
                .call("echo", &current, "{}", Duration::from_secs(2))
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn cancelled_handshake_stops_transport_even_if_the_host_retains_a_guard_reference() {
        let stopped = Arc::new(AtomicBool::new(false));
        let guard = Arc::new(Guard(stopped.clone()));
        let (client, _server) = tokio::io::duplex(1024);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                Connection::from_io(client, guard.clone())
            )
            .await
            .is_err()
        );
        assert!(stopped.load(Ordering::SeqCst));
    }
}
