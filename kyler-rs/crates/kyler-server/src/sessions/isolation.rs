//! Host-owned session directories; never infer filesystem authority from a client path.
use crate::{
    error::{ApiError, ApiResult, valid_id},
    runs::RunManager,
};

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Result, ensure};

use kyler_core::{ExecutionEnvironment, Session};

fn private_directory(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    if !path.exists() {
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let metadata = fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "session directory must not be a symlink"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}
impl RunManager {
    pub(crate) fn isolated_environment(
        &self,
        session: &Session,
    ) -> ApiResult<Option<Arc<ExecutionEnvironment>>> {
        let Some(snapshot) = session
            .environment
            .as_ref()
            .or_else(|| self.agent.workspace_environment())
        else {
            if self.workspaces.is_some() {
                return Err(ApiError::bad(
                    "This session has no authorized cwd; create a new session.",
                ));
            }
            return Ok(None);
        };
        if let Some(managed) = &snapshot.managed {
            let owner = session
                .parent
                .as_ref()
                .map(|parent| parent.session_id.as_str())
                .unwrap_or(&session.session_id);
            valid_id(owner)?;
            let expected = self
                .store
                .directory()
                .join("_execution")
                .join(owner)
                .join("workspace");
            if managed.root != expected
                || !snapshot.cwd.starts_with(&expected)
                || !expected.is_dir()
            {
                return Err(ApiError::bad(
                    "The saved managed directory is missing or its ownership changed.",
                ));
            }
        }
        if let Some(factory) = &self.workspaces {
            return factory
                .restore(snapshot)
                .map(Some)
                .map_err(|_| ApiError::bad("The saved cwd or host sandbox policy changed."));
        }
        let original = self
            .agent
            .execution_environment()
            .ok_or_else(|| ApiError::bad("Session execution environment is unavailable."))?;
        let environment = if let Some(managed) = &snapshot.managed {
            if snapshot.sandbox.base_directory
                != original.session_environment().sandbox.base_directory
            {
                return Err(ApiError::bad("Saved sandbox binding changed."));
            }
            original
                .isolated_session(
                    &managed.root,
                    snapshot
                        .project_root()
                        .unwrap_or(&snapshot.sandbox.base_directory),
                )
                .map_err(ApiError::storage)?
        } else {
            original.as_ref().clone()
        };
        let environment = environment
            .with_cwd(&snapshot.cwd)
            .map_err(ApiError::storage)?
            .with_project_identity(snapshot.project.clone());
        if environment.session_environment() != snapshot {
            return Err(ApiError::bad("The saved cwd or execution binding changed."));
        }
        Ok(Some(Arc::new(environment)))
    }
    /// Explicit directory allocation for isolated sessions and automation tasks, never a cwd side effect.
    pub(crate) fn allocate_session_directory(&self, session: &mut Session) -> ApiResult<()> {
        let Some(base) = self.isolated_environment(session)? else {
            return Ok(());
        };
        if base.session_environment().managed.is_some() {
            return Ok(());
        }
        let owner = session
            .parent
            .as_ref()
            .map(|parent| parent.session_id.as_str())
            .unwrap_or(&session.session_id);
        valid_id(owner)?;
        let execution =
            private_directory(self.store.directory(), "_execution").map_err(ApiError::storage)?;
        let directory = private_directory(&execution, owner).map_err(ApiError::storage)?;
        let root = private_directory(&directory, "workspace").map_err(ApiError::storage)?;
        let project = base
            .session_environment()
            .project_root()
            .unwrap_or(&base.session_environment().cwd);
        let environment = base
            .isolated_session(&root, project)
            .map_err(ApiError::storage)?;
        session.environment = Some(environment.session_environment().clone());
        Ok(())
    }
    pub(crate) fn validate_session_environment(&self, session: &Session) -> ApiResult<()> {
        self.isolated_environment(session)?;
        Ok(())
    }
}
