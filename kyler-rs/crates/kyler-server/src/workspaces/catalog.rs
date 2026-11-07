//! User project names bound to server-authorized canonical directories.
use crate::{
    error::{ApiError, ApiResult, valid_id},
    runs::RunManager,
};

use std::{
    fs,
    io::{ErrorKind, Write},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use axum::http::StatusCode;

use kyler_protocol::api::workspaces::{SaveWorkspaceRequest, WorkspaceInfo};

fn read(path: &Path) -> Result<Vec<WorkspaceInfo>> {
    match fs::read(path) {
        Ok(bytes) => {
            ensure!(
                bytes.len() <= 1024 * 1024,
                "workspace catalog exceeds its limit"
            );
            serde_json::from_slice(&bytes).context("invalid workspace catalog")
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}
impl RunManager {
    pub async fn list_workspaces(&self) -> ApiResult<Vec<WorkspaceInfo>> {
        let _guard = self.workspace_catalog.lock().await;
        let path = self.store.directory().join("_workspaces.json");
        tokio::task::spawn_blocking(move || read(&path))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(ApiError::storage)
    }
    pub async fn save_workspace(&self, request: SaveWorkspaceRequest) -> ApiResult<WorkspaceInfo> {
        let title = request.title.trim().to_owned();
        if title.is_empty() || title.chars().count() > 160 || title.chars().any(char::is_control) {
            return Err(ApiError::bad(
                "Project title must contain 1 to 160 characters without control characters.",
            ));
        }
        let environment = self
            .environment_for_create(Some(&request.path))?
            .ok_or_else(|| ApiError::bad("This server has no configured workspace environment."))?;
        let directory = environment.workspace_root.to_string_lossy().into_owned();
        let _guard = self.workspace_catalog.lock().await;
        let path = self.store.directory().join("_workspaces.json");
        tokio::task::spawn_blocking(move || -> Result<WorkspaceInfo> {
            let mut items = read(&path)?;
            let info = if let Some(item) = items.iter_mut().find(|item| item.path == directory) {
                item.title = title;
                item.clone()
            } else {
                ensure!(items.len() < 256, "workspace catalog is full");
                let info = WorkspaceInfo {
                    id: uuid::Uuid::new_v4().to_string(),
                    path: directory,
                    title,
                };
                items.push(info.clone());
                info
            };
            let parent = path.parent().context("workspace catalog has no parent")?;
            let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
            temporary.write_all(&serde_json::to_vec_pretty(&items)?)?;
            temporary.as_file().sync_all()?;
            temporary.persist(&path).map_err(|e| e.error)?;
            fs::File::open(parent)?.sync_all()?;
            Ok(info)
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(ApiError::storage)
    }
}

impl RunManager {
    /// Remove only the catalog entry. Directories, files and checkpoints are retained.
    pub async fn delete_workspace(&self, id: &str) -> ApiResult<()> {
        valid_id(id)?;
        let _catalog = self.workspace_catalog.lock().await;
        let registry = self.registry.lock().await;
        let path = self.store.directory().join("_workspaces.json");
        let read_path = path.clone();
        let mut items = tokio::task::spawn_blocking(move || read(&read_path))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(ApiError::storage)?;
        let selected = items
            .iter()
            .find(|item| item.id == id)
            .cloned()
            .ok_or_else(ApiError::missing)?;
        let drafts = self.draft_sessions().await?;
        for info in self.store.list().await.map_err(ApiError::storage)? {
            let session = self
                .store
                .load(&info.session_id)
                .await
                .map_err(ApiError::storage)?;
            if session.environment.as_ref().is_some_and(|e| {
                e.project_root.as_deref().unwrap_or(&e.workspace_root) == Path::new(&selected.path)
            }) && (!drafts.contains(&info.session_id)
                || session.is_running()
                || registry.active.contains_key(&info.session_id))
            {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "workspace_not_empty",
                    "Delete the project's chats and stop its active runs before removing it.",
                ));
            }
        }
        items.retain(|item| item.id != id);
        tokio::task::spawn_blocking(move || -> Result<()> {
            let parent = path.parent().context("workspace catalog has no parent")?;
            let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
            temporary.write_all(&serde_json::to_vec_pretty(&items)?)?;
            temporary.as_file().sync_all()?;
            temporary.persist(&path).map_err(|e| e.error)?;
            fs::File::open(parent)?.sync_all()?;
            Ok(())
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(ApiError::storage)
    }
}
