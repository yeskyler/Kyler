//! Trusted workspace binding and immutable sandbox ceilings.
mod worker;
pub mod workspaces;

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

/// Project identity is metadata, never an additional readable or writable root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectIdentity {
    pub root: PathBuf,
}
/// A host binding is revalidated against current host policy on every restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxBinding {
    pub base_directory: PathBuf,
    pub policy_id: String,
}
/// Directory ownership is independent of the command's current directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedDirectory {
    pub root: PathBuf,
}
/// Only cwd is an execution default. Other paths identify metadata and host bindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionEnvironment {
    pub cwd: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed: Option<ManagedDirectory>,
    pub sandbox: SandboxBinding,
}
impl SessionEnvironment {
    pub fn project_root(&self) -> Option<&PathBuf> {
        self.project.as_ref().map(|project| &project.root)
    }
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
            workspace.is_dir() && cwd.is_dir(),
            "cwd must be an existing directory"
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
                project: Some(ProjectIdentity {
                    root: workspace.clone(),
                }),
                managed: None,
                sandbox: SandboxBinding {
                    base_directory: workspace,
                    policy_id: policy.id.clone(),
                },
                cwd,
            },
            policy,
            worker_path: worker,
            backend,
            temporary_directory: temp,
            narrowing_protected_paths: Vec::new(),
        })
    }
    /// Replace grouping metadata without changing execution permissions.
    pub fn with_project_identity(mut self, project: Option<ProjectIdentity>) -> Self {
        self.snapshot.project = project;
        self
    }
    /// Select a default directory without granting additional filesystem access.
    pub fn with_cwd(mut self, cwd: &Path) -> Result<Self> {
        let cwd = self.policy.filesystem.check_read(&self.snapshot.cwd, cwd)?;
        ensure!(cwd.is_dir(), "cwd must be an existing readable directory");
        if let Some(managed) = &self.snapshot.managed {
            ensure!(
                cwd.starts_with(&managed.root),
                "cwd is outside its managed directory"
            );
        }
        self.snapshot.cwd = cwd;
        Ok(self)
    }

    /// Intersect host filesystem authority with this session's execution workspace.
    /// Runtime mounts remain host-controlled; unrelated project/data roots are removed.
    pub fn with_workspace_boundary(&self) -> Result<Self> {
        ensure!(
            self.policy.mode.isolated(),
            "Scoped execution requires an isolated host sandbox"
        );
        let root = self
            .snapshot
            .managed
            .as_ref()
            .map(|managed| &managed.root)
            .or_else(|| self.snapshot.project_root())
            .unwrap_or(&self.snapshot.cwd)
            .canonicalize()?;
        let intersect = |paths: &[PathBuf]| -> Result<Vec<PathBuf>> {
            let mut result = Vec::new();
            for path in paths {
                let path = resolve_path(Path::new("/"), path)?;
                if root.starts_with(&path) {
                    result.push(root.clone());
                } else if path.starts_with(&root) {
                    result.push(path);
                }
            }
            result.sort();
            result.dedup();
            Ok(result)
        };
        let mut environment = self.clone();
        let readonly = self
            .policy
            .filesystem
            .readonly_paths
            .iter()
            .filter(|readonly| {
                // Preserve the original longest-prefix precedence when clamping a broad
                // read-only ancestor and a more specific writable workspace to one root.
                !(root.starts_with(readonly)
                    && self.policy.filesystem.readwrite_paths.iter().any(|write| {
                        root.starts_with(write)
                            && write.components().count() > readonly.components().count()
                    }))
            })
            .cloned()
            .collect::<Vec<_>>();
        environment.policy.filesystem.readonly_paths = intersect(&readonly)?;
        environment.policy.filesystem.readwrite_paths =
            intersect(&self.policy.filesystem.readwrite_paths)?;
        environment.policy.allow_full_access = false;
        environment.policy.validate()?;
        Ok(environment)
    }

    /// Allocate no files: bind an existing private directory within the host policy.
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
            if let Ok(relative) = protected.strip_prefix(&self.snapshot.cwd) {
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
            if let Ok(relative) = denied.strip_prefix(&self.snapshot.cwd) {
                policy.filesystem.denied_paths.push(root.join(relative));
            }
        }
        policy.allow_full_access = self.policy.allow_full_access;
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
        environment.snapshot.project = Some(ProjectIdentity { root: project });
        environment.snapshot.managed = Some(ManagedDirectory { root: root.clone() });
        environment.snapshot.sandbox.base_directory = self.snapshot.sandbox.base_directory.clone();
        Ok(environment)
    }
    /// Build minimal Git metadata without repository hooks, filters or external helpers.
    pub(crate) fn prepare_review_git(
        &self,
        discovery: &crate::subagent::review::git::Discovery,
    ) -> Result<tempfile::TempDir> {
        let filesystem = &self.policy.filesystem;
        let root = &self.snapshot.cwd;
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
        let mut policy = SandboxPolicy::workspace_write(&self.policy.id, &self.snapshot.cwd);
        policy.network = NetworkAccess::AllowOutbound;
        policy.filesystem.readonly_paths.push(PathBuf::from("/"));
        policy.filesystem.denied_paths = self.narrowing_protected_paths.clone();
        for name in [".agents", ".codex", ".kyler/config.toml"] {
            let path = self.snapshot.cwd.join(name);
            policy.filesystem.readonly_paths.push(path.clone());
            policy.filesystem.write_protected_paths.push(path);
        }
        policy.allow_full_access = self.policy.allow_full_access;
        policy.process = self.policy.process.clone();
        policy.runtime_readonly_paths = self.policy.runtime_readonly_paths.clone();
        policy.executable_search_path = self.policy.executable_search_path.clone();
        policy.rustup = self.policy.rustup.clone();
        environment.policy = policy;
        environment.backend = Arc::new(RestrictingSandbox);
        environment
    }
    pub fn allows_full_access(&self) -> bool {
        self.policy.allow_full_access
            && matches!(
                self.policy.mode,
                SandboxMode::WorkspaceWrite | SandboxMode::DangerFullAccess
            )
    }
    /// Select the separately authorized full-access capability without changing directories.
    /// Callers must reject an unavailable selection before executing tools.
    pub fn full_access(&self) -> Self {
        if !self.allows_full_access() {
            return self.clone();
        }
        let mut environment = self.clone();
        let mut policy = SandboxPolicy::danger_full_access(&self.policy.id);
        policy.process = self.policy.process.clone();
        policy.runtime_readonly_paths = self.policy.runtime_readonly_paths.clone();
        policy.executable_search_path = self.policy.executable_search_path.clone();
        policy.rustup = self.policy.rustup.clone();
        environment.policy = policy;
        environment.backend = Arc::new(DirectSandbox);
        environment
    }
    pub fn read_only(&self) -> Self {
        if !self.policy.mode.isolated() {
            return self.workspace_access().read_only();
        }
        let mut policy = self.policy.clone();
        policy.mode = SandboxMode::ReadOnly;
        policy.allow_full_access = false;
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
    fn gateway_workspace_floor_removes_other_roots_without_granting_writes() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("project");
        let other = root.path().join("other");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&other).unwrap();
        std::fs::create_dir(workspace.join(".git")).unwrap();
        let mut policy = SandboxPolicy::workspace_write("gateway-floor", &workspace);
        policy.allow_full_access = true;
        policy
            .filesystem
            .readonly_paths
            .push(std::path::PathBuf::from("/"));
        policy
            .filesystem
            .readonly_paths
            .push(workspace.join(".git"));
        policy.filesystem.readwrite_paths.push(other.clone());
        let original = ExecutionEnvironment::new(
            &workspace,
            &workspace,
            policy,
            std::env::current_exe().unwrap(),
            Arc::new(DirectSandbox),
        )
        .unwrap();
        let narrowed = original.with_workspace_boundary().unwrap();
        let fs = &narrowed.policy().filesystem;
        assert!(
            fs.check_write(&workspace, &workspace.join("result.txt"))
                .is_ok()
        );
        assert!(fs.check_read(&workspace, &other).is_err());
        assert!(
            fs.check_write(&workspace, &other.join("result.txt"))
                .is_err()
        );
        assert!(
            fs.check_write(&workspace, &workspace.join(".git/config"))
                .is_err()
        );
        assert!(!narrowed.allows_full_access());
        assert_eq!(
            original.session_environment(),
            narrowed.session_environment()
        );
        let readonly = original.read_only().with_workspace_boundary().unwrap();
        assert!(
            readonly
                .policy()
                .filesystem
                .check_write(&workspace, &workspace.join("result.txt"))
                .is_err()
        );
        let unrestricted = ExecutionEnvironment::new(
            &workspace,
            &workspace,
            SandboxPolicy::danger_full_access("unrestricted"),
            std::env::current_exe().unwrap(),
            Arc::new(DirectSandbox),
        )
        .unwrap();
        assert!(unrestricted.with_workspace_boundary().is_err());
    }

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
        let own = &isolated.session_environment().cwd;
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

#[cfg(test)]
mod full_access_tests {
    use super::*;

    #[test]
    fn full_access_changes_execution_boundary_only_when_authorized() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let mut policy = SandboxPolicy::workspace_write("fixture", &workspace);
        let environment = ExecutionEnvironment::new(
            &workspace,
            &workspace,
            policy.clone(),
            std::env::current_exe().unwrap(),
            Arc::new(DirectSandbox),
        )
        .unwrap();
        assert_eq!(
            environment.full_access().policy().mode,
            SandboxMode::WorkspaceWrite
        );
        policy.allow_full_access = true;
        let environment = ExecutionEnvironment::new(
            &workspace,
            &workspace,
            policy,
            std::env::current_exe().unwrap(),
            Arc::new(DirectSandbox),
        )
        .unwrap();
        let full = environment.full_access();
        full.policy().validate().unwrap();
        assert_eq!(full.policy().mode, SandboxMode::DangerFullAccess);
        assert_eq!(
            full.session_environment(),
            environment.session_environment()
        );
        assert!(
            environment
                .policy()
                .filesystem
                .check_write(&workspace, &root.path().join("outside"))
                .is_err()
        );
        assert!(
            full.policy()
                .filesystem
                .check_write(&workspace, &root.path().join("outside"))
                .is_ok()
        );
        assert!(matches!(full.policy().network, NetworkAccess::AllowAll));
        assert!(!environment.read_only().allows_full_access());
        assert_eq!(environment.policy().mode, SandboxMode::WorkspaceWrite);
    }
}
