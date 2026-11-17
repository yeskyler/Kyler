//! Draft execution checkpoints stay hidden from history until a model response.
use crate::{
    error::{ApiError, ApiResult},
    runs::{RunManager, persistence},
};

use std::{collections::HashSet, io::ErrorKind};

impl RunManager {
    pub async fn draft_sessions(&self) -> ApiResult<HashSet<String>> {
        let mut entries = tokio::fs::read_dir(&self.root)
            .await
            .map_err(|e| ApiError::storage(e.into()))?;
        let mut ids = HashSet::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ApiError::storage(e.into()))?
        {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "draft") {
                if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
                    ids.insert(id.to_owned());
                }
            }
        }
        Ok(ids)
    }
    pub async fn is_draft(&self, id: &str) -> ApiResult<bool> {
        tokio::fs::try_exists(self.root.join(format!("{id}.draft")))
            .await
            .map_err(|e| ApiError::storage(e.into()))
    }
    pub async fn publish_session(&self, id: &str) -> ApiResult<()> {
        match tokio::fs::remove_file(self.root.join(format!("{id}.draft"))).await {
            Ok(()) => self.update_draft_index(id, false).await,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                self.update_draft_index(id, false).await
            }
            Err(error) => Err(ApiError::storage(error.into())),
        }
    }
    async fn update_draft_index(&self, id: &str, draft: bool) -> ApiResult<()> {
        let index = self.store.catalog();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || index.draft(&id, draft))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(ApiError::storage)
    }
    pub async fn mark_draft(&self, id: &str) -> ApiResult<()> {
        let path = self.root.join(format!("{id}.draft"));
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let file = persistence::private_file(&path)?;
            file.sync_all()?;
            Ok(())
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(ApiError::storage)?;
        self.update_draft_index(id, true).await
    }
}
