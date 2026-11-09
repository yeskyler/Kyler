//! Host-owned session directories; never infer filesystem authority from a client path.
use crate::{
    error::{ApiError, ApiResult, valid_id},
    runs::RunManager,
};

use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
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
                    "This session has no authorized project; create a new session.",
                ));
            }
            return Ok(None);
        };
        if snapshot.workspace_mode == kyler_protocol::api::session::WorkspaceMode::Project {
            if snapshot.project_root.is_some() {
                return Err(ApiError::bad("Project workspace identity mismatch."));
            }
            if let Some(factory) = &self.workspaces {
                return factory
                    .restore(snapshot)
                    .map(Some)
                    .map_err(|_| ApiError::bad("The saved project or sandbox policy changed."));
            }
            let environment = self
                .agent
                .execution_environment()
                .ok_or_else(|| ApiError::bad("Session execution environment is unavailable."))?;
            let project = environment
                .as_ref()
                .clone()
                .project_workspace()
                .map_err(ApiError::storage)?;
            if project.session_environment() != snapshot {
                return Err(ApiError::bad("Session project identity changed."));
            }
            return Ok(Some(Arc::new(project)));
        }
        let owner = session
            .parent
            .as_ref()
            .map(|parent| parent.session_id.as_str())
            .unwrap_or(&session.session_id);
        valid_id(owner)?;
        let root = self
            .store
            .directory()
            .join("_execution")
            .join(owner)
            .join("workspace");
        if snapshot.project_root.is_some()
            && (snapshot.workspace_root != root || snapshot.working_directory != root)
        {
            return Err(ApiError::bad("Session workspace identity mismatch."));
        }
        if snapshot.project_root.is_some() && !root.is_dir() {
            return Err(ApiError::bad(
                "The private session workspace is missing; it was not recreated.",
            ));
        }
        let base = if let Some(factory) = &self.workspaces {
            factory
                .restore(snapshot)
                .map_err(|_| ApiError::bad("The saved workspace or sandbox policy changed."))?
        } else {
            let environment = self
                .agent
                .execution_environment()
                .ok_or_else(|| ApiError::bad("Session execution environment is unavailable."))?;
            if snapshot.project_root.is_some() {
                let project = snapshot.project_root.as_ref().unwrap();
                if project != &environment.session_environment().workspace_root {
                    return Err(ApiError::bad("Session project identity changed."));
                }
                let isolated = environment
                    .isolated_session(&root, project)
                    .map_err(ApiError::storage)?;
                if isolated.session_environment() != snapshot {
                    return Err(ApiError::bad("Session workspace or policy changed."));
                }
                return Ok(Some(Arc::new(isolated)));
            }
            if environment.session_environment() != snapshot {
                return Err(ApiError::bad("Session environment identity mismatch."));
            }
            environment
        };
        if snapshot.project_root.is_some() {
            return Ok(Some(base));
        }
        let execution =
            private_directory(self.store.directory(), "_execution").map_err(ApiError::storage)?;
        let directory = private_directory(&execution, owner).map_err(ApiError::storage)?;
        let workspace = private_directory(&directory, "workspace").map_err(ApiError::storage)?;
        base.isolated_session(&workspace, &snapshot.workspace_root)
            .map(Arc::new)
            .map(Some)
            .map_err(ApiError::storage)
    }
    pub(crate) async fn isolate_session(&self, session: &mut Session) -> ApiResult<bool> {
        let Some(environment) = self.isolated_environment(session)? else {
            return Ok(false);
        };
        if session.environment.as_ref() == Some(environment.session_environment()) {
            return Ok(false);
        }
        // Restore only snapshots owned by this session, never public workspace contents.
        let artifacts = self.artifacts.clone();
        let id = session.session_id.clone();
        let root = environment.session_environment().workspace_root.clone();
        let project = environment
            .session_environment()
            .project_root
            .clone()
            .unwrap();
        let times = session
            .executions
            .iter()
            .chain(session.execution.iter())
            .map(|execution| (execution.execution_id.clone(), execution.started_at))
            .collect::<HashMap<_, _>>();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut files = artifacts.list(&id)?;
            files.sort_by_key(|file| {
                std::cmp::Reverse(times.get(&file.execution_id).copied().unwrap_or_default())
            });
            for file in files.into_iter().filter(|file| file.available) {
                let raw = file.reference.split(['?', '#']).next().unwrap_or_default();
                let Ok(decoded) = percent_encoding::percent_decode_str(raw).decode_utf8() else {
                    continue;
                };
                let path = Path::new(decoded.as_ref());
                let relative = if path.is_absolute() {
                    let Ok(path) = path.strip_prefix(&project) else {
                        continue;
                    };
                    path
                } else {
                    path
                };
                if relative
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
                {
                    continue;
                }
                let Some(name) = relative.file_name() else {
                    continue;
                };
                let mut parent = root.clone();
                if let Some(directory) = relative.parent() {
                    for component in directory.components() {
                        if let Component::Normal(name) = component {
                            parent = private_directory(
                                &parent,
                                name.to_str()
                                    .ok_or_else(|| anyhow::anyhow!("Invalid artifact path"))?,
                            )?;
                        }
                    }
                }
                let destination = parent.join(name);
                if fs::symlink_metadata(&destination).is_ok() {
                    continue;
                }
                let Ok((_, bytes)) = artifacts.download(&id, &file.id) else {
                    tracing::warn!("Unavailable artifact skipped during session isolation");
                    continue;
                };
                let mut output = tempfile::NamedTempFile::new_in(&parent)?;
                output.write_all(&bytes)?;
                output.as_file().sync_all()?;
                match output.persist_noclobber(destination) {
                    Ok(_) => {}
                    Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.error.into()),
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(ApiError::storage)?;
        session.environment = Some(environment.session_environment().clone());
        Ok(true)
    }
}
