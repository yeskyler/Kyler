//! Session ownership and snapshots of configured MCP connections.

use super::{connection::Connection, tool};
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
use kyler_protocol::api::mcp::{McpAction, McpServerInfo, McpStatus};

pub(super) struct Slot {
    pub(super) id: String,
    pub(super) settings: McpServerSettings,
    pub(super) package_root: Option<PathBuf>,
    pub(super) environment: Option<Arc<ExecutionEnvironment>>,
    pub(super) state: RwLock<(McpStatus, Option<Arc<Connection>>, Option<String>)>,
    pub(super) operation: Mutex<()>,
    pub(super) retired: AtomicBool,
}
pub struct McpManager {
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
                Arc::new(Slot {
                    id: id.clone(),
                    settings: settings.clone(),
                    package_root: self.installed_package_root(&settings.command),
                    environment: environment.clone(),
                    state: RwLock::new((
                        if settings.auto_connect {
                            McpStatus::Connecting
                        } else {
                            McpStatus::Disconnected
                        },
                        None,
                        None,
                    )),
                    operation: Mutex::new(()),
                    retired: AtomicBool::new(false),
                })
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
    pub async fn snapshot(
        &self,
        session: &str,
        environment: Option<Arc<ExecutionEnvironment>>,
    ) -> Result<ToolBox> {
        let slots = self.open(session, environment).await?;
        if session != super::system::SYSTEM_SCOPE {
            let connected = self.system_connected().await;
            for slot in &slots {
                if connected.contains(&slot.id)
                    && slot.state.read().unwrap().0 == McpStatus::Disconnected
                {
                    slot.change(McpAction::Connect).await;
                }
            }
        }
        let deadline = Instant::now() + Duration::from_secs(61);
        while slots
            .iter()
            .any(|s| s.state.read().unwrap().0 == McpStatus::Connecting)
            && Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let mut tools = ToolBox::new();
        for slot in slots {
            let state = slot.state.read().unwrap();
            if state.0 != McpStatus::Connected {
                continue;
            }
            if let Some(connection) = &state.1 {
                if connection.service.is_closed() {
                    continue;
                }
                for definition in connection.tools.read().unwrap().iter() {
                    let tool = tool::McpTool::new(
                        &slot.id,
                        definition.clone(),
                        connection.clone(),
                        slot.settings.call_timeout_seconds,
                        &slot.settings.activity,
                    )?;
                    ensure!(!tools.contains_key(tool.name()), "duplicate MCP tool name");
                    tools.insert(tool.name().into(), Arc::new(tool));
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
            slot.retired.store(true, Ordering::SeqCst);
        }
        futures::future::join_all(slots.iter().map(|s| s.change(McpAction::Disconnect))).await;
    }
    pub async fn shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        let sessions = std::mem::take(&mut *self.sessions.lock().await);
        for slot in sessions.values().flatten() {
            slot.retired.store(true, Ordering::SeqCst);
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
