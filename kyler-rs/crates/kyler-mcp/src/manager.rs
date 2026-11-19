//! Lifecycle of one host-scoped connector. Hosts own scope and authorization.
use crate::{Connection, TransportFactory};

use std::{
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::sync::{Mutex, watch};

use kyler_protocol::api::mcp::{McpAction, McpServerInfo, McpStatus, McpToolInfo};

const CONNECTION_FAILURE: &str = "Connection failed. Check the executable, credentials and session sandbox policy, then reconnect.";
const REFRESH_FAILURE: &str = "Connection closed or tool refresh failed; reconnect to retry.";

pub struct Client {
    id: String,
    transport: String,
    factory: Arc<dyn TransportFactory>,
    timeout: Duration,
    changes: watch::Sender<u64>,
    state: RwLock<(McpStatus, Option<Arc<Connection>>, Option<String>)>,
    operation: Mutex<()>,
    retired: AtomicBool,
}
impl Client {
    pub fn new(
        id: String,
        transport: String,
        factory: Arc<dyn TransportFactory>,
        timeout: Duration,
        auto_connect: bool,
        changes: watch::Sender<u64>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            transport,
            factory,
            timeout,
            changes,
            state: RwLock::new((
                if auto_connect {
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
    }
    pub fn status(&self) -> McpStatus {
        self.state.read().unwrap().0
    }
    pub fn connection(&self) -> Option<Arc<Connection>> {
        self.state
            .read()
            .unwrap()
            .1
            .as_ref()
            .filter(|c| c.is_available())
            .cloned()
    }
    pub fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
    }
    pub fn info(&self) -> McpServerInfo {
        let state = self.state.read().unwrap();
        McpServerInfo {
            id: self.id.clone(),
            removable: false,
            transport: self.transport.clone(),
            status: state.0,
            error: state.2.clone(),
            tools: state
                .1
                .as_ref()
                .map(|c| {
                    c.catalog()
                        .tools
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
    fn notify(&self) {
        self.changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    /// Schedule only once when inheriting a host-enabled connector.
    pub fn connect_if_disconnected(self: &Arc<Self>) {
        let mut state = self.state.write().unwrap();
        if state.0 != McpStatus::Disconnected || self.retired.load(Ordering::SeqCst) {
            return;
        }
        state.0 = McpStatus::Connecting;
        drop(state);
        self.notify();
        let client = self.clone();
        tokio::spawn(async move {
            client.change(McpAction::Connect).await;
        });
    }
    pub async fn change(self: &Arc<Self>, action: McpAction) {
        let _operation = self.operation.lock().await;
        if self.retired.load(Ordering::SeqCst) && !matches!(action, McpAction::Disconnect) {
            return;
        }
        if matches!(action, McpAction::Connect) && self.connection().is_some() {
            return;
        }
        let old = {
            let mut state = self.state.write().unwrap();
            state.0 = McpStatus::Disconnected;
            state.2 = None;
            state.1.take()
        };
        if let Some(old) = old {
            old.enabled.store(false, Ordering::SeqCst);
            let drain = async {
                while old.calls.load(Ordering::SeqCst) > 0 {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            };
            let _ = tokio::time::timeout(Duration::from_secs(3), drain).await;
            old.stop();
        }
        if matches!(action, McpAction::Disconnect) {
            self.notify();
            return;
        }
        self.state.write().unwrap().0 = McpStatus::Connecting;
        self.notify();
        match tokio::time::timeout(self.timeout, self.factory.connect()).await {
            Ok(Ok(connection)) => {
                let connection = Arc::new(connection);
                *self.state.write().unwrap() =
                    (McpStatus::Connected, Some(connection.clone()), None);
                let client = Arc::downgrade(self);
                tokio::spawn(async move {
                    while connection.is_available() {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        if client.strong_count() == 0 || !connection.is_available() {
                            break;
                        }
                        if connection.needs_refresh() {
                            match tokio::time::timeout(
                                Duration::from_secs(10),
                                connection.refresh(),
                            )
                            .await
                            {
                                Ok(Ok(())) => {
                                    if let Some(client) = client.upgrade() {
                                        client.notify();
                                    }
                                }
                                _ => break,
                            }
                        }
                    }
                    if let Some(client) = client.upgrade() {
                        let mut state = client.state.write().unwrap();
                        if state
                            .1
                            .as_ref()
                            .is_some_and(|c| Arc::ptr_eq(c, &connection))
                        {
                            *state = (McpStatus::Failed, None, Some(REFRESH_FAILURE.into()));
                            client.notify();
                        }
                    }
                    connection.stop();
                });
            }
            _ => {
                *self.state.write().unwrap() =
                    (McpStatus::Failed, None, Some(CONNECTION_FAILURE.into()))
            }
        }
        self.notify();
    }
}
