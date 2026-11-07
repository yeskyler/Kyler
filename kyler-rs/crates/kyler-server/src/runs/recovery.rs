use super::{manager::RunManager, persistence, registry::Registry};
use crate::error::{ApiError, ApiResult, valid_id};

use std::{io::ErrorKind, time::Duration};

use axum::http::StatusCode;

impl RunManager {
    pub async fn delete_session(&self, id: &str) -> ApiResult<()> {
        valid_id(id)?;
        let mut registry = self.registry.lock().await;
        if registry.active.contains_key(id) {
            return Err(ApiError::conflict());
        }
        let existing = self.store.load(id).await.map_err(ApiError::storage)?;
        if existing.is_running() {
            return Err(ApiError::conflict());
        }
        let ids = registry
            .runs
            .iter()
            .filter(|(_, e)| e.info().session_id == id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            self.remove_locked(&mut registry, &id).await?;
        }
        match tokio::fs::remove_file(self.root.join(format!("{id}.model"))).await {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(ApiError::storage(e.into())),
        }
        match tokio::fs::remove_file(self.root.join(format!("{id}.permissions"))).await {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(ApiError::storage(e.into())),
        }
        if let Some(approvals) = &self.approvals {
            approvals.forget_session(id);
        }
        self.questions
            .forget_session(id)
            .map_err(ApiError::storage)?;
        self.processes.forget_session(id);
        // This derived directory contains only this session's execution files.
        let directory = self.store.directory().join("_execution").join(id);
        if directory.exists()
            && directory
                .canonicalize()
                .map_err(|error| ApiError::storage(error.into()))?
                != directory
        {
            return Err(ApiError::bad(
                "Private execution directory identity changed.",
            ));
        }
        match tokio::fs::symlink_metadata(&directory).await {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                tokio::fs::remove_dir_all(&directory)
                    .await
                    .map_err(|error| ApiError::storage(error.into()))?
            }
            Ok(_) => {
                return Err(ApiError::bad(
                    "Private execution directory identity changed.",
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(ApiError::storage(error.into())),
        }
        self.store.delete(id).await.map_err(ApiError::storage)?;
        self.publish_session(id).await?;
        self.attachments
            .delete_session(id)
            .map_err(ApiError::storage)?;
        self.artifacts
            .delete_session(id)
            .map_err(ApiError::storage)?;
        if let Some(manager) = self.agent.mcp_manager() {
            manager.close_session(id).await;
        }
        Ok(())
    }
    pub(super) async fn remove_locked(&self, registry: &mut Registry, id: &str) -> ApiResult<()> {
        persistence::remove(&self.root, id)
            .await
            .map_err(ApiError::storage)?;
        registry.runs.remove(id);
        if let Some(approvals) = &self.approvals {
            approvals.forget_run(id);
        }
        registry.keys.retain(|_, (run, _)| run != id);
        Ok(())
    }
    pub(super) async fn prune_locked(&self, registry: &mut Registry, room: bool) -> ApiResult<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let mut terminal = registry
            .runs
            .values()
            .filter_map(|e| {
                let info = e.info();
                info.finished_at.map(|time| (info.run_id, time))
            })
            .collect::<Vec<_>>();
        terminal.sort_by_key(|e| e.1);
        for (id, time) in terminal {
            if now.saturating_sub(time)
                > self.settings.completed_run_retention_seconds as i64 * 1000
                || (room && registry.runs.len() >= self.settings.max_retained_runs)
            {
                self.remove_locked(registry, &id).await?;
            }
        }
        if room && registry.runs.len() >= self.settings.max_retained_runs {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "capacity_exceeded",
                "The run registry is full.",
            ));
        }
        Ok(())
    }
    pub async fn cleanup(&self) -> ApiResult<()> {
        let mut registry = self.registry.lock().await;
        self.prune_locked(&mut registry, false).await
    }
    pub async fn shutdown(&self) -> bool {
        self.stopping.send_replace(true);
        let drain = async {
            {
                let registry = self.registry.lock().await;
                for entry in registry.runs.values() {
                    if !entry.info().status.is_terminal() {
                        if let Some(approvals) = &self.approvals {
                            approvals.cancel_run(&entry.info().run_id);
                        }
                        entry.cancel.send_replace(true);
                    }
                }
            }
            loop {
                if self.registry.lock().await.active.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            self.agent.shutdown().await;
        };
        tokio::time::timeout(
            Duration::from_secs(self.settings.shutdown_grace_seconds),
            drain,
        )
        .await
        .is_ok()
    }
}
