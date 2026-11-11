//! MCP transport establishment, discovery and connection lifecycle.

use super::{manager::Slot, process, tool};
use crate::sandbox::NetworkAccess;

use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, ensure};
use rmcp::{
    ClientHandler, ServiceExt,
    model::Tool as RemoteTool,
    service::{NotificationContext, RoleClient, RunningService},
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};

use kyler_config::McpTransport;
use kyler_protocol::api::mcp::{McpAction, McpServerInfo, McpStatus, McpToolInfo};

#[derive(Clone)]
pub(super) struct Handler(Arc<AtomicBool>);
impl ClientHandler for Handler {
    async fn on_tool_list_changed(&self, _: NotificationContext<RoleClient>) {
        self.0.store(true, Ordering::SeqCst);
    }
}
pub(super) struct Connection {
    pub(super) approval_id: String,
    pub(super) service: RunningService<RoleClient, Handler>,
    pub(super) _process: Option<process::ProcessLease>,
    pub(super) enabled: AtomicBool,
    pub(super) calls: AtomicUsize,
    pub(super) changed: Arc<AtomicBool>,
    pub(super) tools: RwLock<Vec<RemoteTool>>,
}

impl Slot {
    pub(super) fn info(&self) -> McpServerInfo {
        let state = self.state.read().unwrap();
        McpServerInfo {
            removable: false,
            id: self.id.clone(),
            transport: match self.settings.transport {
                McpTransport::Stdio => "stdio",
                McpTransport::StreamableHttp => "streamable_http",
            }
            .into(),
            status: state.0,
            error: state.2.clone(),
            tools: state
                .1
                .as_ref()
                .map(|c| {
                    c.tools
                        .read()
                        .unwrap()
                        .iter()
                        .map(|t| McpToolInfo {
                            name: t.name.to_string(),
                            description: t
                                .description
                                .as_deref()
                                .unwrap_or_default()
                                .chars()
                                .take(512)
                                .collect(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
    pub(super) async fn change(self: &Arc<Self>, action: McpAction) {
        self.change_inner(action).await;
        self.changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    async fn change_inner(self: &Arc<Self>, action: McpAction) {
        let _operation = self.operation.lock().await;
        if self.retired.load(Ordering::SeqCst) && !matches!(action, McpAction::Disconnect) {
            return;
        }
        if matches!(action, McpAction::Connect)
            && self
                .state
                .read()
                .unwrap()
                .1
                .as_ref()
                .is_some_and(|c| !c.service.is_closed())
        {
            return;
        }
        let old = {
            let mut state = self.state.write().unwrap();
            state.0 = McpStatus::Disconnected;
            state.2 = None;
            state.1.take()
        };
        if let Some(connection) = old {
            connection.enabled.store(false, Ordering::SeqCst);
            let drain = async {
                while connection.calls.load(Ordering::SeqCst) > 0 {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            };
            let _ = tokio::time::timeout(Duration::from_secs(3), drain).await;
            connection.service.cancellation_token().cancel();
            if let Some(process) = &connection._process {
                process.stop();
            }
        }
        if matches!(action, McpAction::Disconnect) {
            return;
        }
        let denied = match (&self.settings.transport, &self.environment) {
            (McpTransport::Stdio, None) => Some("Stdio MCP requires a session workspace sandbox."),
            (McpTransport::Stdio, Some(environment))
                if !environment.policy().process.allow_commands =>
            {
                Some(
                    "Session policy denies command execution. Enable --allow-commands to connect a stdio MCP server.",
                )
            }
            (McpTransport::StreamableHttp, Some(environment))
                if matches!(environment.policy().network, NetworkAccess::Deny) =>
            {
                Some(
                    "Session policy denies network access. Enable --allow-network to connect an HTTP MCP server.",
                )
            }
            _ => None,
        };
        if let Some(message) = denied {
            *self.state.write().unwrap() = (McpStatus::Failed, None, Some(message.into()));
            return;
        }
        self.state.write().unwrap().0 = McpStatus::Connecting;
        self.changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
        let connecting = tokio::time::timeout(
            Duration::from_secs(self.settings.connect_timeout_seconds),
            self.connect(),
        )
        .await;
        match connecting {
            Ok(Ok(connection)) => {
                let connection = Arc::new(connection);
                *self.state.write().unwrap() = (McpStatus::Connected, Some(connection.clone()), None);
                let slot = Arc::downgrade(self);
                tokio::spawn(async move {
                    while connection.enabled.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        if slot.strong_count() == 0 || connection.service.is_closed() { break; }
                        if connection.changed.swap(false, Ordering::SeqCst) {
                            match tokio::time::timeout(Duration::from_secs(10), discover(&connection.service)).await {
                                Ok(Ok(tools)) => {
                                    *connection.tools.write().unwrap() = tools;
                                    if let Some(slot) = slot.upgrade() {slot.changes.send_modify(|revision| *revision = revision.wrapping_add(1));}
                                },
                                _ => { connection.enabled.store(false, Ordering::SeqCst); break; }
                            }
                        }
                    }
                    if let Some(slot) = slot.upgrade() {
                        let mut state = slot.state.write().unwrap();
                        if state.1.as_ref().is_some_and(|c| Arc::ptr_eq(c, &connection)) {
                            *state = (McpStatus::Failed, None, Some("Connection closed or tool refresh failed; reconnect to retry.".into()));
                            slot.changes.send_modify(|revision| *revision = revision.wrapping_add(1));
                        }
                    }
                    connection.service.cancellation_token().cancel();
                    if let Some(process) = &connection._process { process.stop(); }
                });
            }
            _ => *self.state.write().unwrap() = (McpStatus::Failed, None, Some("Connection failed. Check the executable, credentials and session sandbox policy, then reconnect.".into())),
        }
    }

    async fn connect(&self) -> Result<Connection> {
        let changed = Arc::new(AtomicBool::new(false));
        let handler = Handler(changed.clone());
        let (service, process) = match self.settings.transport {
            McpTransport::Stdio => {
                let environment = self.environment.clone().ok_or_else(|| {
                    anyhow::anyhow!("stdio MCP requires an authorized sandbox environment")
                })?;
                let mut vars = BTreeMap::new();
                for (key, _reference) in &self.settings.env {
                    vars.insert(
                        key.clone(),
                        self.settings
                            .resolved_env
                            .get(key)
                            .ok_or_else(|| anyhow::anyhow!("MCP environment reference is missing"))?
                            .expose()
                            .to_owned(),
                    );
                }
                let (transport, lease) = process::launch(
                    &self.settings.command,
                    &self.settings.args,
                    vars,
                    environment,
                    self.package_root.as_deref(),
                )
                .await?;
                (handler.serve(transport).await?, Some(lease))
            }
            McpTransport::StreamableHttp => {
                let mut http = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(Duration::from_secs(10));
                if let Some(environment) = &self.environment {
                    match &environment.policy().network {
                        NetworkAccess::Deny => {
                            anyhow::bail!("sandbox network policy denies MCP HTTP")
                        }
                        NetworkAccess::Proxy { url } => {
                            http = http.proxy(reqwest::Proxy::all(url)?)
                        }
                        NetworkAccess::AllowOutbound
                        | NetworkAccess::AllowAll
                        | NetworkAccess::Inherit => {}
                    }
                }
                let mut options =
                    StreamableHttpClientTransportConfig::with_uri(self.settings.url.clone());
                if let Some(_reference) = &self.settings.bearer_token_env {
                    options = options.auth_header(
                        self.settings
                            .resolved_token
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("MCP token reference is missing"))?
                            .expose(),
                    );
                }
                // Do not replay a tools/call after a session expires.
                options = options.reinit_on_expired_session(false);
                (
                    handler
                        .serve(StreamableHttpClientTransport::with_client(
                            http.build()?,
                            options,
                        ))
                        .await?,
                    None,
                )
            }
        };
        let tools = discover(&service).await?;
        Ok(Connection {
            approval_id: uuid::Uuid::new_v4().to_string(),
            service,
            _process: process,
            enabled: AtomicBool::new(true),
            calls: AtomicUsize::new(0),
            changed,
            tools: RwLock::new(tools),
        })
    }
}
pub(super) async fn discover(
    service: &RunningService<RoleClient, Handler>,
) -> Result<Vec<RemoteTool>> {
    let mut tools = Vec::new();
    let mut cursor = None;
    let mut seen = HashSet::new();
    loop {
        let mut params = rmcp::model::PaginatedRequestParams::default();
        params.cursor = cursor;
        let page = service.list_tools(Some(params)).await?;
        ensure!(
            tools.len() + page.tools.len() <= 256,
            "MCP tool count exceeds 256"
        );
        for tool in page.tools {
            ensure!(
                seen.insert(tool.name.to_string()),
                "duplicate MCP tool name"
            );
            tool::validate_name(&tool.name)?;
            ensure!(
                serde_json::to_vec(&tool)?.len() <= 64 * 1024,
                "MCP tool schema exceeds 64 KiB"
            );
            tools.push(tool);
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        ensure!(tools.len() < 256, "MCP pagination exceeds limit");
    }
    Ok(tools)
}
