//! Trusted workspace binding and immutable sandbox ceilings.
mod worker;

use crate::sandbox::{
    MxcSandbox, NetworkAccess, Sandbox, SandboxMode, SandboxPolicy,
    direct::{DirectSandbox, RestrictingSandbox},
    paths::resolve_path,
};

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

/// Persisted directory context and policy identity, never the authority to grant access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEnvironment {
    /// Optional project grouping; it does not grant filesystem access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<PathBuf>,
    #[serde(
        default,
        skip_serializing_if = "kyler_protocol::api::session::WorkspaceMode::is_isolated"
    )]
    pub workspace_mode: kyler_protocol::api::session::WorkspaceMode,
    pub workspace_root: PathBuf,
    pub working_directory: PathBuf,
    pub sandbox_policy_id: String,
}
/// Immutable, host-authorized environment shared by parent and children.
#[derive(Clone)]
pub struct ExecutionEnvironment {
    pub(crate) snapshot: SessionEnvironment,
    policy: SandboxPolicy,
    worker_path: PathBuf,
    backend: Arc<dyn Sandbox>,
    temporary_directory: Arc<tempfile::TempDir>,
    narrowing_protected_paths: Vec<PathBuf>,
}
impl ExecutionEnvironment {
    pub fn mxc(
        workspace: impl AsRef<Path>,
        cwd: impl AsRef<Path>,
        policy: SandboxPolicy,
        worker: impl AsRef<Path>,
    ) -> Result<Self> {
        let backend: Arc<dyn Sandbox> = if policy.mode.isolated() {
            Arc::new(MxcSandbox::new()?)
        } else {
            Arc::new(DirectSandbox)
        };
        Self::new(workspace, cwd, policy, worker, backend)
    }
    pub fn new(
        workspace: impl AsRef<Path>,
        cwd: impl AsRef<Path>,
        mut policy: SandboxPolicy,
        worker: impl AsRef<Path>,
        backend: Arc<dyn Sandbox>,
    ) -> Result<Self> {
        let workspace = std::fs::canonicalize(workspace)?;
        let cwd = resolve_path(&workspace, cwd.as_ref())?;
        ensure!(
            workspace.is_dir() && cwd.is_dir() && cwd.starts_with(&workspace),
            "working directory must be an existing directory in the workspace"
        );
        policy.validate()?;
        policy.filesystem.normalize()?;
        policy.validate()?;
        policy.filesystem.check_read(&workspace, &cwd)?;
        policy.runtime_readonly_paths = policy
            .runtime_readonly_paths
            .iter()
            .map(std::fs::canonicalize)
            .collect::<std::io::Result<_>>()?;
        let worker = std::fs::canonicalize(worker).context(
            "tool worker missing; run cargo build -p kyler-core --bin kyler-tool-worker",
        )?;
        ensure!(worker.is_file(), "tool worker must be an executable file");
        let temp = Arc::new(
            tempfile::Builder::new()
                .prefix("kyler-sandbox-")
                .tempdir()?,
        );
        if let Some(rustup) = &policy.rustup {
            let bin = temp.path().join("bin");
            std::fs::create_dir(&bin)?;
            std::fs::create_dir(temp.path().join(".cargo"))?;
            for name in [
                "rustup",
                "cargo",
                "rustc",
                "rustdoc",
                "rustfmt",
                "cargo-fmt",
                "cargo-clippy",
                "clippy-driver",
                "rust-analyzer",
            ] {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&rustup.binary, bin.join(name))?;
                #[cfg(not(unix))]
                std::fs::copy(&rustup.binary, bin.join(name))?;
            }
            policy.executable_search_path =
                format!("{}:{}", bin.display(), policy.executable_search_path);
        }
        Ok(Self {
            snapshot: SessionEnvironment {
                project_root: None,
                workspace_mode: Default::default(),
                workspace_root: workspace,
                working_directory: cwd,
                sandbox_policy_id: policy.id.clone(),
            },
            policy,
            worker_path: worker,
            backend,
            temporary_directory: temp,
            narrowing_protected_paths: Vec::new(),
        })
    }
    /// Rebind execution to a private directory. Project identity never becomes a readable root.
    /// Keep execution in the host-authorized project instead of a private copy.
    pub fn project_workspace(mut self) -> Result<Self> {
        ensure!(
            self.snapshot.project_root.is_none(),
            "an isolated workspace cannot become a project workspace"
        );
        ensure!(
            self.snapshot.workspace_root == self.snapshot.working_directory,
            "project workspace must execute at its root"
        );
        self.snapshot.workspace_mode = kyler_protocol::api::session::WorkspaceMode::Project;
        Ok(self)
    }

    pub fn isolated_session(&self, directory: &Path, project: &Path) -> Result<Self> {
        let root = directory.canonicalize()?;
        let project = project.canonicalize()?;
        let mut policy = if self.policy.mode == SandboxMode::ReadOnly {
            SandboxPolicy::read_only(&self.policy.id, &root)
        } else {
            SandboxPolicy::workspace_write(&self.policy.id, &root)
        };
        policy.id = format!("{}:session-isolation-v1", self.policy.id);
        for protected in &self.policy.filesystem.write_protected_paths {
            if let Ok(relative) = protected.strip_prefix(&self.snapshot.workspace_root) {
                policy.filesystem.readonly_paths.push(root.join(relative));
                policy
                    .filesystem
                    .write_protected_paths
                    .push(root.join(relative));
            }
        }
        policy
            .filesystem
            .denied_paths
            .extend(self.policy.filesystem.denied_paths.clone());
        policy
            .filesystem
            .denied_paths
            .extend(self.narrowing_protected_paths.clone());
        for denied in &self.policy.filesystem.denied_paths {
            if let Ok(relative) = denied.strip_prefix(&self.snapshot.workspace_root) {
                policy.filesystem.denied_paths.push(root.join(relative));
            }
        }
        policy.process = self.policy.process.clone();
        if policy.mode != SandboxMode::ReadOnly {
            policy.network = match &self.policy.network {
                NetworkAccess::AllowAll | NetworkAccess::Inherit => NetworkAccess::AllowOutbound,
                network => network.clone(),
            };
        }
        policy.runtime_readonly_paths = self.policy.runtime_readonly_paths.clone();
        ensure!(
            policy
                .runtime_readonly_paths
                .iter()
                .all(|path| !root.starts_with(path)),
            "runtime roots cannot contain session workspaces"
        );
        policy.executable_search_path = self.policy.executable_search_path.clone();
        policy.rustup = self.policy.rustup.clone();
        let backend = if self.policy.mode.isolated() {
            self.backend.clone()
        } else {
            Arc::new(RestrictingSandbox)
        };
        let mut environment = Self::new(&root, &root, policy, &self.worker_path, backend)?;
        environment.snapshot.project_root = Some(project);
        Ok(environment)
    }
    /// Build minimal Git metadata without repository hooks, filters or external helpers.
    pub(crate) fn prepare_review_git(
        &self,
        discovery: &crate::subagent::review::git::Discovery,
    ) -> Result<tempfile::TempDir> {
        let filesystem = &self.policy.filesystem;
        let root = &self.snapshot.workspace_root;
        let objects = filesystem.check_read(root, &discovery.common_dir.join("objects"))?;
        let directory = tempfile::Builder::new()
            .prefix("review-git-")
            .tempdir_in(self.temporary_directory.path())?;
        std::fs::create_dir(directory.path().join("refs"))?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(objects, directory.path().join("objects"))?;
        #[cfg(not(unix))]
        anyhow::bail!("Review Git metadata currently requires Unix filesystem links");
        let config = if discovery.object_format == "sha256" {
            "[core]\nrepositoryformatversion=1\nbare=false\n[extensions]\nobjectFormat=sha256\n"
        } else {
            "[core]\nrepositoryformatversion=0\nbare=false\n"
        };
        std::fs::write(directory.path().join("config"), config)?;
        std::fs::write(
            directory.path().join("HEAD"),
            format!("{}\n", discovery.head),
        )?;
        let index = discovery.git_dir.join("index");
        if index.exists() {
            let index = filesystem.check_read(root, &index)?;
            ensure!(
                index.metadata()?.len() <= 16 * 1024 * 1024,
                "Review Git index exceeds 16 MiB"
            );
            std::fs::copy(index, directory.path().join("index"))?;
        }
        let exclude = discovery.common_dir.join("info/exclude");
        if exclude.exists() {
            let exclude = filesystem.check_read(root, &exclude)?;
            ensure!(
                exclude.metadata()?.len() <= 65536,
                "Review Git exclude file exceeds 64 KiB"
            );
            std::fs::create_dir(directory.path().join("info"))?;
            std::fs::copy(exclude, directory.path().join("info/exclude"))?;
        }
        Ok(directory)
    }
    pub(crate) fn attachment_cache_directory(&self) -> PathBuf {
        self.temporary_directory.path().join("attachment-cache")
    }
    pub fn session_environment(&self) -> &SessionEnvironment {
        &self.snapshot
    }
    /// Narrow a host-authorized ceiling without changing persisted workspace identity.
    pub fn with_protected_paths(&self, paths: &[PathBuf]) -> Result<Self> {
        let mut policy = self.policy.clone();
        if !policy.mode.isolated() {
            let mut environment = self.clone();
            for path in paths {
                environment
                    .narrowing_protected_paths
                    .push(resolve_path(Path::new("/"), path)?);
            }
            return Ok(environment);
        }
        for path in paths {
            policy
                .filesystem
                .denied_paths
                .push(resolve_path(Path::new("/"), path)?);
        }
        policy.filesystem.denied_paths.sort();
        policy.filesystem.denied_paths.dedup();
        Ok(Self {
            snapshot: self.snapshot.clone(),
            policy,
            worker_path: self.worker_path.clone(),
            backend: self.backend.clone(),
            temporary_directory: self.temporary_directory.clone(),
            narrowing_protected_paths: self.narrowing_protected_paths.clone(),
        })
    }
    /// Narrow an unrestricted host to workspace writes and outbound networking.
    pub fn workspace_access(&self) -> Self {
        if self.policy.mode.isolated() {
            return self.clone();
        }
        let mut environment = self.clone();
        let mut policy =
            SandboxPolicy::workspace_write(&self.policy.id, &self.snapshot.workspace_root);
        policy.network = NetworkAccess::AllowOutbound;
        policy.filesystem.readonly_paths.push(PathBuf::from("/"));
        policy.filesystem.denied_paths = self.narrowing_protected_paths.clone();
        for name in [".git", ".agents", ".codex", ".kyler/config.toml"] {
            let path = self.snapshot.workspace_root.join(name);
            policy.filesystem.readonly_paths.push(path.clone());
            policy.filesystem.write_protected_paths.push(path);
        }
        policy.process = self.policy.process.clone();
        policy.runtime_readonly_paths = self.policy.runtime_readonly_paths.clone();
        policy.executable_search_path = self.policy.executable_search_path.clone();
        policy.rustup = self.policy.rustup.clone();
        environment.policy = policy;
        environment.backend = Arc::new(RestrictingSandbox);
        environment
    }
    pub fn read_only(&self) -> Self {
        if !self.policy.mode.isolated() {
            return self.workspace_access().read_only();
        }
        let mut policy = self.policy.clone();
        policy.mode = SandboxMode::ReadOnly;
        policy
            .filesystem
            .readonly_paths
            .extend(policy.filesystem.readwrite_paths.drain(..));
        policy.network = NetworkAccess::Deny;
        policy.process.allow_commands = false;
        Self {
            snapshot: self.snapshot.clone(),
            policy,
            worker_path: self.worker_path.clone(),
            backend: if self.policy.mode.isolated() {
                self.backend.clone()
            } else {
                Arc::new(RestrictingSandbox)
            },
            temporary_directory: self.temporary_directory.clone(),
            narrowing_protected_paths: self.narrowing_protected_paths.clone(),
        }
    }
    pub fn policy(&self) -> &SandboxPolicy {
        &self.policy
    }
}

#[cfg(test)]
mod isolation_tests {
    use super::ExecutionEnvironment;
    use crate::sandbox::{SandboxMode, SandboxPolicy, direct::DirectSandbox};
    use std::sync::Arc;

    #[test]
    fn private_sessions_cannot_inherit_unrestricted_or_shared_filesystem_roots() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let private = root.path().join("private");
        let sibling = root.path().join("sibling");
        for directory in [&project, &private, &sibling] {
            std::fs::create_dir(directory).unwrap();
        }
        let base = ExecutionEnvironment::new(
            &project,
            &project,
            SandboxPolicy::danger_full_access("host-full-access"),
            std::env::current_exe().unwrap(),
            Arc::new(DirectSandbox),
        )
        .unwrap();
        let isolated = base.isolated_session(&private, &project).unwrap();
        assert_eq!(isolated.policy().mode, SandboxMode::WorkspaceWrite);
        let own = &isolated.session_environment().workspace_root;
        assert!(
            isolated
                .policy()
                .filesystem
                .check_write(own, &own.join("result.txt"))
                .is_ok()
        );
        assert!(
            isolated
                .policy()
                .filesystem
                .check_read(own, &project)
                .is_err()
        );
        assert!(
            isolated
                .policy()
                .filesystem
                .check_read(own, &sibling)
                .is_err()
        );
        assert!(
            isolated
                .workspace_access()
                .policy()
                .filesystem
                .check_read(own, &project)
                .is_err()
        );
        assert!(
            isolated
                .read_only()
                .policy()
                .filesystem
                .check_write(own, &own.join("result.txt"))
                .is_err()
        );
    }
}
