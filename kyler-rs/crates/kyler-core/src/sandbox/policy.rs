use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub use kyler_protocol::sandbox::SandboxMode;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FilesystemPolicy {
    pub readonly_paths: Vec<PathBuf>,
    pub readwrite_paths: Vec<PathBuf>,
    pub denied_paths: Vec<PathBuf>,
    #[serde(default)]
    pub write_protected_paths: Vec<PathBuf>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum NetworkAccess {
    #[default]
    Deny,
    /// No local network filtering in explicit unrestricted mode.
    AllowAll,
    /// Network restrictions belong to the operator-provided external sandbox.
    Inherit,
    /// Outbound internet only; host-loopback remains denied.
    AllowOutbound,
    /// Destination filtering belongs to this host-managed HTTP proxy.
    Proxy { url: String },
}
#[derive(Debug, Clone)]
pub struct ProcessPolicy {
    pub allow_commands: bool,
    pub timeout_ms: u32,
    pub background_timeout_ms: u32,
    pub max_output_bytes: usize,
    pub max_request_bytes: usize,
}
impl Default for ProcessPolicy {
    fn default() -> Self {
        Self {
            allow_commands: true,
            timeout_ms: 30_000,
            background_timeout_ms: 600_000,
            max_output_bytes: 1024 * 1024,
            max_request_bytes: 1024 * 1024,
        }
    }
}
/// Host-installed runtime paths; never populated from model tool arguments.
#[derive(Debug, Clone, Serialize)]
pub struct RustupRuntime {
    pub home: PathBuf,
    pub binary: PathBuf,
    pub toolchain: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    pub mode: SandboxMode,
    pub id: String,
    pub filesystem: FilesystemPolicy,
    pub network: NetworkAccess,
    pub process: ProcessPolicy,
    /// Explicit runtime locations, e.g. a ripgrep executable and its libraries.
    pub runtime_readonly_paths: Vec<PathBuf>,
    pub executable_search_path: String,
    pub rustup: Option<RustupRuntime>,
}
impl SandboxPolicy {
    pub fn read_only(id: impl Into<String>, workspace: impl AsRef<Path>) -> Self {
        Self {
            mode: SandboxMode::ReadOnly,
            id: id.into(),
            filesystem: FilesystemPolicy {
                readonly_paths: vec![workspace.as_ref().into()],
                ..Default::default()
            },
            network: NetworkAccess::Deny,
            process: ProcessPolicy::default(),
            runtime_readonly_paths: Vec::new(),
            executable_search_path: "/usr/bin:/bin:/usr/sbin:/sbin".into(),
            rustup: None,
        }
    }
    pub fn workspace_write(id: impl Into<String>, workspace: impl AsRef<Path>) -> Self {
        let mut policy = Self::read_only(id, workspace.as_ref());
        policy.mode = SandboxMode::WorkspaceWrite;
        policy.filesystem.readonly_paths.clear();
        policy
            .filesystem
            .readwrite_paths
            .push(workspace.as_ref().into());
        policy
    }
    pub fn danger_full_access(id: impl Into<String>) -> Self {
        let mut policy = Self::workspace_write(id, Path::new("/"));
        policy.mode = SandboxMode::DangerFullAccess;
        policy.network = NetworkAccess::AllowAll;
        policy
    }
    pub fn external_sandbox(id: impl Into<String>) -> Self {
        let mut policy = Self::danger_full_access(id);
        policy.mode = SandboxMode::ExternalSandbox;
        policy.network = NetworkAccess::Inherit;
        policy
    }
    pub fn validate(&self) -> Result<()> {
        if self.mode == SandboxMode::ReadOnly {
            ensure!(
                self.filesystem.readwrite_paths.is_empty()
                    && matches!(self.network, NetworkAccess::Deny),
                "read-only sandbox must deny writes and network access"
            );
        }
        if !self.mode.isolated() {
            ensure!(
                self.filesystem.readonly_paths.is_empty()
                    && self.filesystem.denied_paths.is_empty()
                    && self.filesystem.write_protected_paths.is_empty()
                    && self.filesystem.readwrite_paths == [PathBuf::from("/")],
                "unrestricted and external modes cannot include local filesystem restrictions"
            );
        }
        if self.mode == SandboxMode::DangerFullAccess {
            ensure!(
                matches!(self.network, NetworkAccess::AllowAll),
                "danger-full-access requires unrestricted network access"
            );
        }
        if self.mode == SandboxMode::ExternalSandbox {
            ensure!(
                matches!(self.network, NetworkAccess::Inherit),
                "external-sandbox must inherit external network policy"
            );
        }
        ensure!(
            !self.id.trim().is_empty(),
            "sandbox policy ID cannot be empty"
        );
        ensure!(
            self.process.timeout_ms > 0
                && self.process.background_timeout_ms > 0
                && self.process.max_output_bytes > 0
                && self.process.max_request_bytes > 0,
            "sandbox timeout and output/request bounds must be positive"
        );
        ensure!(
            self.process.max_output_bytes <= 64 * 1024 * 1024
                && self.process.max_request_bytes <= 64 * 1024 * 1024,
            "sandbox message bounds cannot exceed 64 MiB"
        );
        if let NetworkAccess::Proxy { url } = &self.network {
            let url = reqwest::Url::parse(url)?;
            ensure!(
                matches!(url.scheme(), "http" | "https")
                    && matches!(
                        url.host_str(),
                        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
                    )
                    && url.port().is_some(),
                "proxy must be a host-managed loopback HTTP(S) URL with explicit port"
            );
        }
        for protected in &self.filesystem.write_protected_paths {
            ensure!(
                self.filesystem.readonly_paths.contains(protected),
                "protected paths must also be present in readonly_paths"
            );
        }
        for writable in &self.filesystem.readwrite_paths {
            ensure!(
                !self
                    .filesystem
                    .write_protected_paths
                    .iter()
                    .any(|protected| writable.starts_with(protected)),
                "additional write root overlaps a protected path"
            );
        }
        if let Some(rustup) = &self.rustup {
            ensure!(
                rustup.home.is_dir() && rustup.binary.is_file(),
                "host Rustup runtime is unavailable"
            );
            ensure!(
                rustup.home.is_absolute() && rustup.binary.is_absolute(),
                "Rustup runtime paths must be absolute"
            );
            ensure!(
                self.runtime_readonly_paths
                    .iter()
                    .any(|root| rustup.home.starts_with(root))
                    && self
                        .runtime_readonly_paths
                        .iter()
                        .any(|root| rustup.binary.starts_with(root)),
                "Rustup runtime must be explicitly included in readonly runtime paths"
            );
            ensure!(
                rustup
                    .toolchain
                    .as_ref()
                    .is_none_or(|value| !value.contains('\0') && value.len() <= 4096),
                "invalid host toolchain selection"
            );
        }
        for path in self
            .filesystem
            .readonly_paths
            .iter()
            .chain(&self.filesystem.readwrite_paths)
            .chain(&self.filesystem.denied_paths)
            .chain(&self.filesystem.write_protected_paths)
            .chain(&self.runtime_readonly_paths)
        {
            ensure!(
                path.is_absolute(),
                "sandbox policy paths must be absolute: {}",
                path.display()
            );
        }
        Ok(())
    }
}
