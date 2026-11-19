//! Session ownership and snapshots of configured MCP connections.

use super::{connection::HostTransport, tool};
use crate::{
    environment::ExecutionEnvironment,
    tools::{Tool, ToolBox},
};

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, ensure};
use tokio::{sync::Mutex, time::Instant};

use kyler_config::{McpServerSettings, McpSettings, McpTransport};
use kyler_mcp::Client;
use kyler_protocol::api::mcp::{McpAction, McpServerInfo, McpStatus};

pub(super) struct Slot {
    pub(super) id: String,
    pub(super) settings: McpServerSettings,
    pub(super) environment: Option<Arc<ExecutionEnvironment>>,
    pub(super) client: Arc<Client>,
}
impl Slot {
    pub(super) fn new(
        id: String,
        settings: McpServerSettings,
        environment: Option<Arc<ExecutionEnvironment>>,
        package_root: Option<PathBuf>,
        changes: tokio::sync::watch::Sender<u64>,
    ) -> Self {
        let transport = match settings.transport {
            kyler_config::McpTransport::Stdio => "stdio",
            kyler_config::McpTransport::StreamableHttp => "streamable_http",
        };
        let factory = Arc::new(HostTransport {
            settings: settings.clone(),
            environment: environment.clone(),
            package_root,
        });
        let client = Client::new(
            id.clone(),
            transport.into(),
            factory,
            Duration::from_secs(settings.connect_timeout_seconds),
            settings.auto_connect,
            changes,
        );
        Self {
            id,
            settings,
            environment,
            client,
        }
    }
    pub(super) fn info(&self) -> McpServerInfo {
        self.client.info()
    }
    pub(super) async fn change(&self, action: McpAction) {
        self.client.change(action).await;
    }
}
pub struct McpManager {
    pub(super) changes: tokio::sync::watch::Sender<u64>,
    pub(super) configured_ids: HashSet<String>,
    pub(super) settings: RwLock<McpSettings>,
    pub(super) system_environment: std::sync::OnceLock<Arc<ExecutionEnvironment>>,
    pub(super) system_enabled: Mutex<HashSet<String>>,
    pub(super) user_servers: Mutex<McpSettings>,
    pub(super) user_store: Option<PathBuf>,
    pub(super) sessions: Mutex<HashMap<String, Vec<Arc<Slot>>>>,
    pub(super) stopping: AtomicBool,
}
impl McpManager {
    pub fn new(settings: McpSettings) -> Result<Self> {
        settings.validate()?;
        settings.validate_credentials()?;
        Ok(Self {
            changes: tokio::sync::watch::channel(0).0,
            configured_ids: settings.servers.keys().cloned().collect(),
            settings: RwLock::new(settings),
            system_environment: std::sync::OnceLock::new(),
            system_enabled: Mutex::new(HashSet::new()),
            user_servers: Mutex::new(McpSettings::default()),
            user_store: None,
            sessions: Mutex::new(HashMap::new()),
            stopping: AtomicBool::new(false),
        })
    }
    /// Local subscribers refresh snapshots only when connector state changes.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// Definitions only: no process, connection, or environment is allocated.
    pub fn configured_servers(&self) -> Result<Vec<McpServerInfo>> {
        let settings = self
            .settings
            .read()
            .map_err(|_| anyhow::anyhow!("MCP settings unavailable"))?;
        Ok(settings
            .servers
            .iter()
            .filter(|(_, server)| server.enabled)
            .map(|(id, server)| McpServerInfo {
                removable: false,
                id: id.clone(),
                transport: match server.transport {
                    McpTransport::Stdio => "stdio",
                    McpTransport::StreamableHttp => "streamable_http",
                }
                .into(),
                status: McpStatus::Disconnected,
                tools: Vec::new(),
                error: None,
            })
            .collect())
    }
    async fn open(
        &self,
        session: &str,
        environment: Option<Arc<ExecutionEnvironment>>,
    ) -> Result<Vec<Arc<Slot>>> {
        ensure!(
            !self.stopping.load(Ordering::SeqCst),
            "MCP manager is stopping"
        );
        let mut sessions = self.sessions.lock().await;
        if let Some(slots) = sessions.get(session) {
            for slot in slots {
                ensure!(
                    slot.environment.as_ref().map(|e| e.session_environment())
                        == environment.as_ref().map(|e| e.session_environment()),
                    "MCP workspace binding changed"
                );
            }
            return Ok(slots.clone());
        }
        if self
            .settings
            .read()
            .map_err(|_| anyhow::anyhow!("MCP settings unavailable"))?
            .servers
            .values()
            .all(|s| !s.enabled)
        {
            return Ok(Vec::new());
        }
        ensure!(sessions.len() < 128, "MCP session capacity reached");
        let settings = self
            .settings
            .read()
            .map_err(|_| anyhow::anyhow!("MCP settings unavailable"))?;
        let slots: Vec<_> = settings
            .servers
            .iter()
            .filter(|(_, s)| s.enabled)
            .map(|(id, settings)| {
                Arc::new(Slot::new(
                    id.clone(),
                    settings.clone(),
                    environment.clone(),
                    self.installed_package_root(&settings.command),
                    self.changes.clone(),
                ))
            })
            .collect();
        drop(settings);
        sessions.insert(session.into(), slots.clone());
        for slot in &slots {
            if slot.settings.auto_connect {
                let slot = slot.clone();
                tokio::spawn(async move {
                    slot.change(McpAction::Connect).await;
                });
            }
        }
        Ok(slots)
    }
    pub async fn list(
        &self,
        session: &str,
        environment: Option<Arc<ExecutionEnvironment>>,
    ) -> Result<Vec<McpServerInfo>> {
        Ok(self
            .open(session, environment)
            .await?
            .iter()
            .map(|s| s.info())
            .collect())
    }
    /// Fingerprint only this scope's connection/catalog generations, never another account's.
    pub(crate) async fn scope_revision(&self, session: &str) -> String {
        use sha2::{Digest, Sha256};
        let sessions = self.sessions.lock().await;
        let mut hash = Sha256::new();
        for slot in sessions.get(session).into_iter().flatten() {
            hash.update(slot.id.as_bytes());
            hash.update([0]);
            if let Some(connection) = slot.client.connection() {
                hash.update(connection.approval_id().as_bytes());
                hash.update(connection.catalog().version.as_bytes());
            }
            hash.update([0]);
        }
        format!("{hash:x}", hash = hash.finalize())
    }
    /// Lightweight availability for retrieval; does not clone tool schemas/descriptions.
    pub(crate) async fn connection_states(
        &self,
        session: &str,
        environment: Option<Arc<ExecutionEnvironment>>,
    ) -> Result<Vec<(String, McpStatus)>> {
        Ok(self
            .open(session, environment)
            .await?
            .iter()
            .map(|slot| (slot.id.clone(), slot.client.status()))
            .collect())
    }
    pub async fn action(
        &self,
        session: &str,
        environment: Option<Arc<ExecutionEnvironment>>,
        id: &str,
        action: McpAction,
    ) -> Result<()> {
        let slots = self.open(session, environment).await?;
        let slot = slots
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow::anyhow!("MCP server is not configured or enabled"))?;
        // Schedule before returning: never hold an HTTP request open during a handshake.
        tokio::spawn(async move {
            slot.change(action).await;
        });
        Ok(())
    }
    async fn ready_slots(
        &self,
        session: &str,
        environment: Option<Arc<ExecutionEnvironment>>,
    ) -> Result<Vec<Arc<Slot>>> {
        let slots = self.open(session, environment).await?;
        if session != super::system::SYSTEM_SCOPE {
            let connected = self.system_connected().await;
            for slot in &slots {
                if connected.contains(&slot.id) {
                    slot.client.connect_if_disconnected();
                }
            }
        }
        Ok(slots)
    }
    /// Immediately returns only currently connected tools. Never waits for a handshake.
    pub async fn snapshot_ready(
        &self,
        session: &str,
        environment: Option<Arc<ExecutionEnvironment>>,
    ) -> Result<ToolBox> {
        Self::tools_from_slots(self.ready_slots(session, environment).await?)
    }
    /// Compatibility API for callers explicitly waiting for initial discovery.
    pub async fn snapshot(
        &self,
        session: &str,
        environment: Option<Arc<ExecutionEnvironment>>,
    ) -> Result<ToolBox> {
        let slots = self.ready_slots(session, environment).await?;
        let deadline = Instant::now() + Duration::from_secs(61);
        while slots
            .iter()
            .any(|slot| slot.client.status() == McpStatus::Connecting)
            && Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Self::tools_from_slots(slots)
    }
    /// Only explicit connector mentions use this bounded wait; it grants no connection authority.
    pub async fn wait_for_servers(
        &self,
        session: &str,
        environment: Option<Arc<ExecutionEnvironment>>,
        ids: &[String],
        timeout: Duration,
    ) -> Result<()> {
        let slots = self.ready_slots(session, environment).await?;
        let deadline = Instant::now() + timeout;
        while slots
            .iter()
            .any(|slot| ids.contains(&slot.id) && slot.client.status() == McpStatus::Connecting)
            && Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }
    pub fn search_settings(&self) -> kyler_config::McpSearchSettings {
        self.settings.read().unwrap().search.clone()
    }
    fn tools_from_slots(slots: Vec<Arc<Slot>>) -> Result<ToolBox> {
        let mut tools = ToolBox::new();
        for slot in slots {
            if let Some(connection) = slot.client.connection() {
                let catalog = connection.catalog();
                for definition in &catalog.tools {
                    let tool = tool::McpTool::new(
                        &slot.id,
                        definition.clone(),
                        connection.clone(),
                        slot.settings.call_timeout_seconds,
                        &slot.settings.activity,
                        &catalog.version,
                    )?;
                    ensure!(!tools.contains_key(tool.name()), "duplicate MCP tool name");
                    tools.insert(tool.name().into(), Arc::new(tool) as Arc<dyn Tool>);
                }
            }
        }
        Ok(tools)
    }
    pub async fn close_session(&self, session: &str) {
        let slots = self
            .sessions
            .lock()
            .await
            .remove(session)
            .unwrap_or_default();
        for slot in &slots {
            slot.client.retire();
        }
        futures::future::join_all(slots.iter().map(|s| s.change(McpAction::Disconnect))).await;
    }
    pub async fn shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        let sessions = std::mem::take(&mut *self.sessions.lock().await);
        for slot in sessions.values().flatten() {
            slot.client.retire();
        }
        futures::future::join_all(
            sessions
                .values()
                .flatten()
                .map(|s| s.change(McpAction::Disconnect)),
        )
        .await;
    }
}
