//! System plugin discovery connections are isolated from project execution scopes.
use super::McpManager;
use crate::{
    environment::ExecutionEnvironment,
    sandbox::{NetworkAccess, SandboxMode, SandboxPolicy},
};

use std::{collections::HashSet, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};

use kyler_protocol::api::mcp::{McpAction, McpServerInfo, McpStatus};

pub(super) const SYSTEM_SCOPE: &str = "__kyler_system_plugins";

impl McpManager {
    pub(super) fn package_directory(&self) -> Result<PathBuf> {
        Ok(self
            .user_store
            .as_ref()
            .and_then(|path| path.parent())
            .context("Connector persistence is unavailable")?
            .join("_connector_packages"))
    }
    pub(super) fn installed_package_root(&self, command: &str) -> Option<PathBuf> {
        let directory = self.package_directory().ok()?.canonicalize().ok()?;
        let command = PathBuf::from(command).canonicalize().ok()?;
        let id = command.strip_prefix(&directory).ok()?.components().next()?;
        Some(directory.join(id.as_os_str()))
    }
    fn system_environment(&self) -> Result<Arc<ExecutionEnvironment>> {
        if let Some(environment) = self.system_environment.get() {
            return Ok(environment.clone());
        }
        let directory = self.package_directory()?;
        std::fs::create_dir_all(&directory)?;
        let directory = directory.canonicalize()?;
        let mut policy = SandboxPolicy::read_only("system-connectors", &directory);
        policy.mode = SandboxMode::WorkspaceWrite;
        policy.network = NetworkAccess::AllowOutbound;
        // Runtime installations are readable; project directories and user homes are not.
        for runtime in [
            "/usr",
            "/bin",
            "/sbin",
            "/System",
            "/Library/Frameworks",
            "/Library/Developer/CommandLineTools",
            "/opt/homebrew",
        ] {
            let path = PathBuf::from(runtime);
            if path.exists() {
                policy.runtime_readonly_paths.push(path);
            }
        }
        let environment = Arc::new(ExecutionEnvironment::mxc(
            &directory,
            &directory,
            policy,
            std::env::current_exe()?,
        )?);
        let _ = self.system_environment.set(environment);
        Ok(self.system_environment.get().unwrap().clone())
    }
    pub async fn system_list(&self) -> Result<Vec<McpServerInfo>> {
        let mut servers = self
            .list(SYSTEM_SCOPE, Some(self.system_environment()?))
            .await?;
        let user = self.user_servers.lock().await;
        for server in &mut servers {
            server.removable =
                user.servers.contains_key(&server.id) && !self.configured_ids.contains(&server.id);
        }
        Ok(servers)
    }
    pub async fn system_action(&self, id: &str, action: McpAction) -> Result<()> {
        let _user = self.user_servers.lock().await;
        self.action(SYSTEM_SCOPE, Some(self.system_environment()?), id, action)
            .await?;
        {
            let mut enabled = self.system_enabled.lock().await;
            if matches!(action, McpAction::Disconnect) {
                enabled.remove(id);
            } else {
                enabled.insert(id.to_owned());
            }
        }
        if matches!(action, McpAction::Disconnect | McpAction::Reconnect) {
            let slots = self
                .sessions
                .lock()
                .await
                .iter()
                .filter(|(scope, _)| scope.as_str() != SYSTEM_SCOPE)
                .flat_map(|(_, slots)| slots)
                .filter(|slot| slot.id == id)
                .cloned()
                .collect::<Vec<_>>();
            for slot in slots {
                slot.change(McpAction::Disconnect).await;
            }
        }
        Ok(())
    }
    pub(super) async fn system_connected(&self) -> HashSet<String> {
        let enabled = self.system_enabled.lock().await.clone();
        self.sessions
            .lock()
            .await
            .get(SYSTEM_SCOPE)
            .into_iter()
            .flatten()
            .filter(|slot| {
                enabled.contains(&slot.id) && slot.state.read().unwrap().0 == McpStatus::Connected
            })
            .map(|slot| slot.id.clone())
            .collect()
    }
}
