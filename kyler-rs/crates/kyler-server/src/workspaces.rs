//! Host-owned policy for isolated, session-specific workspaces.
mod catalog;
mod runtime;

use self::runtime::Runtime;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};

use kyler_config::SandboxSettings;
use kyler_core::{
    ExecutionEnvironment, SessionEnvironment,
    sandbox::{
        MxcSandbox, NetworkAccess, Sandbox, SandboxPolicy, direct::DirectSandbox,
        policy::RustupRuntime,
    },
    tools::{ToolBox, command::RunCommandTool},
};
use kyler_protocol::sandbox::SandboxMode;

#[derive(Clone)]
pub struct WorkspaceFactory {
    command_timeout_ms: u32,
    background_timeout_ms: u32,
    worker: PathBuf,
    allowed_roots: Vec<PathBuf>,
    writable: bool,
    commands: bool,
    network: bool,
    backend: Arc<dyn Sandbox>,
    runtime_paths: Vec<PathBuf>,
    search_path: String,
    rustup: Option<RustupRuntime>,
    sandbox: SandboxSettings,
}
impl WorkspaceFactory {
    pub fn native(
        worker: PathBuf,
        allowed_roots: Vec<PathBuf>,
        writable: bool,
        commands: bool,
        network: bool,
    ) -> Result<Self> {
        let sandbox = SandboxSettings {
            mode: if writable {
                SandboxMode::WorkspaceWrite
            } else {
                SandboxMode::ReadOnly
            },
            allow_commands: commands,
            network_access: network,
            ..Default::default()
        };
        Self::configured(worker, allowed_roots, sandbox)
    }
    pub fn configured(
        worker: PathBuf,
        allowed_roots: Vec<PathBuf>,
        mut sandbox: SandboxSettings,
    ) -> Result<Self> {
        sandbox.validate()?;
        sandbox.resolve_paths(
            &std::env::current_dir()?,
            std::env::var_os("HOME").as_deref().map(Path::new),
        )?;
        let writable = sandbox.mode != SandboxMode::ReadOnly;
        let commands = sandbox.allow_commands;
        let network = !sandbox.mode.isolated() || sandbox.network_access;
        let worker = worker.canonicalize().context("worker executable missing")?;
        ensure!(worker.is_file(), "worker must be a file");
        let allowed_roots = allowed_roots
            .iter()
            .map(std::fs::canonicalize)
            .collect::<std::io::Result<Vec<_>>>()?;
        ensure!(
            allowed_roots.iter().all(|path| path.is_dir()),
            "allowed roots must be directories"
        );
        let runtime = Runtime::discover();
        let mut search_path = runtime.search_path();
        if !sandbox.mode.isolated() {
            search_path = std::env::var("PATH").unwrap_or(search_path);
        }
        Ok(Self {
            command_timeout_ms: 30000,
            background_timeout_ms: 600000,
            worker,
            allowed_roots,
            writable,
            commands,
            network,
            backend: if sandbox.mode.isolated() {
                Arc::new(MxcSandbox::new()?)
            } else {
                Arc::new(DirectSandbox)
            },
            sandbox,
            runtime_paths: runtime.readonly_paths,
            search_path,
            rustup: runtime.rustup,
        })
    }
    /// The configured default task directory is an additional host-authorized root.
    pub fn with_default_workspace(mut self, directory: &Path) -> Result<Self> {
        let directory = directory.canonicalize()?;
        ensure!(directory.is_dir(), "default workspace must be a directory");
        if !self.allowed_roots.is_empty() && !self.allowed_roots.contains(&directory) {
            self.allowed_roots.push(directory);
        }
        Ok(self)
    }
    pub fn with_process_timeouts(mut self, command_ms: u32, background_ms: u32) -> Result<Self> {
        ensure!(
            (1..=3600000).contains(&command_ms) && (1..=3600000).contains(&background_ms),
            "Invalid command timeouts"
        );
        self.command_timeout_ms = command_ms;
        self.background_timeout_ms = background_ms;
        Ok(self)
    }
    pub fn sandbox_mode(&self) -> SandboxMode {
        self.sandbox.mode
    }
    pub fn session_sandbox_mode(&self) -> SandboxMode {
        if self.sandbox.mode == SandboxMode::ReadOnly {
            SandboxMode::ReadOnly
        } else {
            SandboxMode::WorkspaceWrite
        }
    }
    pub fn unrestricted_roots(&self) -> bool {
        self.allowed_roots.is_empty()
    }
    pub fn filter_tools(&self, tools: &mut ToolBox) {
        if !self.writable {
            tools.remove("apply_patch");
        }
        if self.commands {
            tools.insert("run_command".into(), Arc::new(RunCommandTool));
        }
    }
    pub fn create(&self, workspace: &Path) -> Result<Arc<ExecutionEnvironment>> {
        self.build(workspace, workspace)
    }
    fn build(&self, workspace: &Path, cwd: &Path) -> Result<Arc<ExecutionEnvironment>> {
        ensure!(
            workspace.is_absolute() && cwd.is_absolute(),
            "workspace paths must be absolute"
        );
        let root = workspace.canonicalize().context("workspace not found")?;
        ensure!(root.is_dir(), "workspace must be a directory");
        ensure!(
            self.allowed_roots.is_empty()
                || self
                    .allowed_roots
                    .iter()
                    .any(|allowed| root.starts_with(allowed)),
            "workspace is outside the allowed roots"
        );
        let mut policy = match self.sandbox.mode {
            SandboxMode::ReadOnly => SandboxPolicy::read_only("cli-read-only-v2", &root),
            SandboxMode::WorkspaceWrite => {
                SandboxPolicy::workspace_write("cli-workspace-write-v2", &root)
            }
            SandboxMode::DangerFullAccess => {
                SandboxPolicy::danger_full_access("cli-danger-full-access-v1")
            }
            SandboxMode::ExternalSandbox => {
                SandboxPolicy::external_sandbox("cli-external-sandbox-v1")
            }
        };
        let protected = self
            .sandbox
            .protected_paths
            .iter()
            .map(|path| {
                if path.is_absolute() {
                    path.clone()
                } else {
                    root.join(path)
                }
            })
            .collect::<Vec<_>>();
        if policy.mode.isolated() {
            policy
                .filesystem
                .readonly_paths
                .extend(self.sandbox.additional_read_roots.clone());
            if self.writable {
                policy
                    .filesystem
                    .readwrite_paths
                    .extend(self.sandbox.additional_write_roots.clone());
                // Keep repository metadata and assistant configuration immutable in normal workspaces.
                policy.filesystem.readonly_paths.extend(
                    [".git", ".agents", ".codex", ".kyler/config.toml"].map(|path| root.join(path)),
                );
                policy.filesystem.readonly_paths.extend(protected.clone());
                policy.filesystem.write_protected_paths.extend(
                    [".git", ".agents", ".codex", ".kyler/config.toml"].map(|path| root.join(path)),
                );
                policy
                    .filesystem
                    .write_protected_paths
                    .extend(protected.clone());
            }
            if self.network {
                policy.network = NetworkAccess::AllowOutbound;
            }
        }
        // A policy revision binds persisted sessions to the complete host-selected boundary.
        let boundary = serde_json::json!({"mode":self.sandbox.mode,"commands":self.commands,"network":self.network,"read":policy.filesystem.readonly_paths,"write":policy.filesystem.readwrite_paths,"protected":self.sandbox.protected_paths,"runtime_readonly_paths":self.runtime_paths,"rustup":self.rustup});
        policy.id = format!("{}:{}", policy.id, boundary);
        policy.process.allow_commands = self.commands;
        policy.process.timeout_ms = self.command_timeout_ms;
        policy.process.background_timeout_ms = self.background_timeout_ms;
        policy.runtime_readonly_paths = self.runtime_paths.clone();
        policy.executable_search_path = self.search_path.clone();
        policy.rustup = self.rustup.clone();
        Ok(Arc::new(ExecutionEnvironment::new(
            &root,
            cwd,
            policy,
            &self.worker,
            self.backend.clone(),
        )?))
    }
    pub fn restore(&self, snapshot: &SessionEnvironment) -> Result<Arc<ExecutionEnvironment>> {
        let restore = |factory: &Self| -> Result<Arc<ExecutionEnvironment>> {
            if let Some(project) = &snapshot.project_root {
                return Ok(Arc::new(
                    factory
                        .create(project)?
                        .isolated_session(&snapshot.workspace_root, project)?,
                ));
            }
            let environment =
                factory.build(&snapshot.workspace_root, &snapshot.working_directory)?;
            if snapshot.workspace_mode == kyler_protocol::api::session::WorkspaceMode::Project {
                return Ok(Arc::new(environment.as_ref().clone().project_workspace()?));
            }
            Ok(environment)
        };
        let environment = restore(self)?;
        if environment.session_environment() != snapshot
            && self.sandbox.mode == SandboxMode::WorkspaceWrite
            && self.network
        {
            // Preserve existing offline sessions when only the new host default changed.
            let mut offline = self.clone();
            offline.network = false;
            offline.sandbox.network_access = false;
            let previous = restore(&offline)?;
            if previous.session_environment() == snapshot {
                return Ok(previous);
            }
        }
        ensure!(
            environment.session_environment() == snapshot,
            "session workspace or policy changed; create a new session"
        );
        Ok(environment)
    }
}
