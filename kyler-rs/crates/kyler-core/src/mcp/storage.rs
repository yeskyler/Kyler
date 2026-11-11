//! User-created connector definitions. Credentials remain environment references.
use super::manager::{McpManager, Slot};

use std::{
    collections::HashMap,
    fs,
    io::{ErrorKind, Write},
    path::PathBuf,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, ensure};
use tokio::sync::Mutex;

use kyler_config::{McpServerSettings, McpSettings, McpTransport};
use kyler_protocol::api::mcp::{McpServerInfo, McpStatus};

impl McpManager {
    pub fn with_user_store(mut self, path: PathBuf) -> Result<Self> {
        let mut user = match fs::read(&path) {
            Ok(bytes) => {
                ensure!(
                    bytes.len() <= 1024 * 1024,
                    "connector definitions exceed 1 MiB"
                );
                serde_json::from_slice::<McpSettings>(&bytes).context("invalid saved connectors")?
            }
            Err(error) if error.kind() == ErrorKind::NotFound => McpSettings::default(),
            Err(error) => return Err(error.into()),
        };
        user.resolve_environment(&std::env::vars().collect::<HashMap<_, _>>());
        user.validate()?;
        user.validate_credentials()?;
        let configured = self
            .settings
            .get_mut()
            .map_err(|_| anyhow::anyhow!("MCP settings unavailable"))?;
        for (id, server) in &user.servers {
            configured
                .servers
                .entry(id.clone())
                .or_insert_with(|| server.clone());
        }
        configured.validate()?;
        self.user_servers = Mutex::new(user);
        self.user_store = Some(path);
        Ok(self)
    }

    /// Remove a user definition, retire every session connection, and clean its owned package.
    pub async fn remove_user_connector(&self, id: &str) -> Result<()> {
        ensure!(
            !self.configured_ids.contains(id),
            "configuration-defined connectors cannot be uninstalled"
        );
        let path = self
            .user_store
            .clone()
            .context("connector persistence is unavailable")?;
        let mut user = self.user_servers.lock().await;
        let removed = user
            .servers
            .get(id)
            .cloned()
            .context("user connector not found")?;
        let mut next = user.clone();
        next.servers.remove(id);
        let bytes = serde_json::to_vec_pretty(&next)?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let parent = path
                .parent()
                .context("connector store needs a parent directory")?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(&bytes)?;
            file.as_file().sync_all()?;
            file.persist(&path).map_err(|error| error.error)?;
            fs::File::open(parent)?.sync_all()?;
            Ok(())
        })
        .await??;
        let mut retired = Vec::new();
        {
            let mut sessions = self.sessions.lock().await;
            self.settings
                .write()
                .map_err(|_| anyhow::anyhow!("MCP settings unavailable"))?
                .servers
                .remove(id);
            for slots in sessions.values_mut() {
                for slot in slots.iter().filter(|slot| slot.id == id) {
                    slot.retired.store(true, Ordering::SeqCst);
                    retired.push(slot.clone());
                }
                slots.retain(|slot| slot.id != id);
            }
        }
        self.system_enabled.lock().await.remove(id);
        *user = next;
        futures::future::join_all(
            retired
                .iter()
                .map(|slot| slot.change(kyler_protocol::api::mcp::McpAction::Disconnect)),
        )
        .await;
        if let Some(root) = self.installed_package_root(&removed.command) {
            let shared = self
                .settings
                .read()
                .map_err(|_| anyhow::anyhow!("MCP settings unavailable"))?
                .servers
                .values()
                .any(|server| self.installed_package_root(&server.command).as_ref() == Some(&root));
            // Never remove an arbitrary executable directory or another connector's files.
            if !shared && root.file_name().and_then(|name| name.to_str()) == Some(id) {
                tokio::task::spawn_blocking(move || -> Result<()> {
                    ensure!(
                        fs::symlink_metadata(&root)?.is_dir()
                            && !fs::symlink_metadata(&root)?.file_type().is_symlink(),
                        "invalid connector package directory"
                    );
                    let parent = root.parent().context("connector package has no parent")?;
                    let trash = tempfile::Builder::new()
                        .prefix(".connector-remove-")
                        .tempdir_in(parent)?;
                    fs::rename(&root, trash.path().join("package"))?;
                    trash.close()?;
                    Ok(())
                })
                .await??;
            }
        }
        self.changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
        Ok(())
    }

    /// Publish a new definition without connecting or replacing an existing one.
    pub async fn add_user_connector(
        &self,
        id: String,
        mut server: McpServerSettings,
    ) -> Result<McpServerInfo> {
        ensure!(
            !self.stopping.load(Ordering::SeqCst),
            "MCP manager is stopping"
        );
        let path = self
            .user_store
            .clone()
            .context("connector persistence is unavailable")?;
        server.enabled = true;
        server.auto_connect = false;
        let mut user = self.user_servers.lock().await;
        let mut next = user.clone();
        next.servers.insert(id.clone(), server);
        next.resolve_environment(&std::env::vars().collect::<HashMap<_, _>>());
        next.validate()?;
        next.validate_credentials()?;
        let server = next.servers[&id].clone();
        let mut configured = self
            .settings
            .read()
            .map_err(|_| anyhow::anyhow!("MCP settings unavailable"))?
            .clone();
        ensure!(
            !configured.servers.contains_key(&id),
            "connector ID already exists"
        );
        configured.servers.insert(id.clone(), server.clone());
        configured.validate()?;
        let bytes = serde_json::to_vec_pretty(&next)?;
        ensure!(
            bytes.len() <= 1024 * 1024,
            "connector definitions exceed 1 MiB"
        );
        tokio::task::spawn_blocking(move || -> Result<()> {
            let parent = path
                .parent()
                .context("connector store needs a parent directory")?;
            fs::create_dir_all(parent)?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(&bytes)?;
            file.as_file().sync_all()?;
            file.persist(&path).map_err(|e| e.error)?;
            fs::File::open(parent)?.sync_all()?;
            Ok(())
        })
        .await??;
        let mut sessions = self.sessions.lock().await;
        *self
            .settings
            .write()
            .map_err(|_| anyhow::anyhow!("MCP settings unavailable"))? = configured;
        for slots in sessions.values_mut() {
            let environment = slots.first().and_then(|s| s.environment.clone());
            slots.push(Arc::new(Slot {
                changes: self.changes.clone(),
                id: id.clone(),
                settings: server.clone(),
                package_root: self.installed_package_root(&server.command),
                environment,
                state: RwLock::new((McpStatus::Disconnected, None, None)),
                operation: Mutex::new(()),
                retired: AtomicBool::new(false),
            }));
        }
        *user = next;
        self.changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
        Ok(McpServerInfo {
            removable: true,
            id,
            transport: match server.transport {
                McpTransport::Stdio => "stdio",
                McpTransport::StreamableHttp => "streamable_http",
            }
            .into(),
            status: McpStatus::Disconnected,
            tools: Vec::new(),
            error: None,
        })
    }
}
