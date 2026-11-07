//! Session preferences are separate from execution checkpoints so active runs retain their lease.
use crate::{
    error::{ApiError, ApiResult, valid_id},
    runs::{RunManager, persistence},
};

use std::{
    fs::File,
    io::{ErrorKind, Write},
    path::PathBuf,
};

use kyler_config::loader::LlmOverrides;
use kyler_protocol::api::models::{ModelCatalog, ModelSelection, SessionModel};

impl RunManager {
    /// Only trusted startup code chooses the reload source, never API input.
    pub fn set_model_source(&self, path: PathBuf, overrides: LlmOverrides) {
        *self.model_source.write().expect("model source poisoned") = Some((path, overrides));
    }
    pub fn model_catalog(&self) -> ApiResult<ModelCatalog> {
        self.agent
            .model_registry()
            .map(|r| r.catalog())
            .ok_or_else(|| {
                ApiError::bad("Model management is unavailable for this embedded runtime.")
            })
    }
    pub async fn reload_models(&self) -> ApiResult<ModelCatalog> {
        let _reload = self.model_reload.lock().await;
        let source = self
            .model_source
            .read()
            .map_err(|_| ApiError::bad("Model source unavailable."))?
            .clone()
            .ok_or_else(|| ApiError::bad("No model configuration reload source was registered."))?;
        let config = tokio::task::spawn_blocking(move || {
            kyler_config::loader::load_with_llm_overrides(Some(&source.0), &source.1)
        })
        .await
        .map_err(|_| ApiError::bad("Model reload failed."))?
        .map_err(|_| {
            ApiError::bad("Invalid model configuration. The previous catalog remains active.")
        })?;
        self.agent.model_registry().ok_or_else(|| ApiError::bad("Model management is unavailable."))?
            .reload(&config).map_err(|_| ApiError::bad("Invalid model budgets or provider configuration. The previous catalog remains active."))
    }
    pub(crate) async fn read_model_selection(&self, id: &str) -> ApiResult<ModelSelection> {
        let path = self.root.join(format!("{id}.model"));
        match tokio::fs::read(path).await {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| ApiError::bad("Invalid saved model selection.")),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Default::default()),
            Err(e) => Err(ApiError::storage(e.into())),
        }
    }
    pub async fn session_model(
        &self,
        id: &str,
        selection: Option<ModelSelection>,
    ) -> ApiResult<SessionModel> {
        valid_id(id)?;
        let registry = self.registry.lock().await;
        self.store.load(id).await.map_err(ApiError::storage)?;
        let models = self
            .agent
            .model_registry()
            .ok_or_else(|| ApiError::bad("Model management is unavailable."))?;
        let selection = if let Some(selection) = selection {
            if selection.profile.as_ref().is_some_and(|p| {
                p.is_empty()
                    || p.len() > 128
                    || !p
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            }) {
                return Err(ApiError::bad("Invalid model profile name."));
            }
            models
                .resolve(selection.profile.as_deref())
                .map_err(|e| ApiError::bad(&e.to_string()))?;
            let path = self.root.join(format!("{id}.model"));
            let root = self.root.clone();
            let bytes = serde_json::to_vec(&selection)
                .map_err(|_| ApiError::bad("Cannot encode model selection."))?;
            tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let temporary = root.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
                let result = (|| -> anyhow::Result<()> {
                    let mut file = persistence::private_file(&temporary)?;
                    file.write_all(&bytes)?;
                    file.sync_all()?;
                    std::fs::rename(&temporary, path)?;
                    File::open(&root)?.sync_all()?;
                    Ok(())
                })();
                if result.is_err() {
                    let _ = std::fs::remove_file(&temporary);
                }
                result
            })
            .await
            .map_err(|_| ApiError::bad("Cannot save model selection."))?
            .map_err(ApiError::storage)?;
            selection
        } else {
            self.read_model_selection(id).await?
        };
        let active = registry
            .active
            .get(id)
            .and_then(|run| registry.runs.get(run))
            .and_then(|entry| entry.info().model);
        let resolved = models.resolve(selection.profile.as_deref());
        let (selected, error) = match resolved {
            Ok(model) => (Some(model.info.clone()), None),
            Err(e) => (None, Some(e.to_string())),
        };
        Ok(SessionModel {
            profile: selection.profile,
            selected,
            active,
            error,
        })
    }
}
