//! Session preferences are separate from execution checkpoints so active runs retain their lease.
use crate::{
    error::{ApiError, ApiResult, valid_id},
    runs::RunManager,
};

use std::{io::ErrorKind, path::PathBuf};

use kyler_config::loader::LlmOverrides;
use kyler_protocol::api::models::{ModelCatalog, ModelSelection, SessionModel};

impl RunManager {
    pub async fn session_thinking(
        &self,
        id: &str,
        enabled: Option<bool>,
    ) -> ApiResult<kyler_protocol::api::models::ThinkingState> {
        valid_id(id)?;
        let _registry = self.registry.lock().await;
        let session = self.store.load(id).await.map_err(ApiError::storage)?;
        self.core_thread(&session)
            .await?
            .handle
            .thinking(enabled)
            .await
            .map_err(|error| ApiError::bad(&error.to_string()))
    }
    pub async fn session_thinking_level(
        &self,
        id: &str,
        level: kyler_protocol::api::models::ThinkingLevel,
    ) -> ApiResult<kyler_protocol::api::models::ThinkingState> {
        valid_id(id)?;
        let _registry = self.registry.lock().await;
        let session = self.store.load(id).await.map_err(ApiError::storage)?;
        self.core_thread(&session)
            .await?
            .handle
            .thinking_level(Some(level))
            .await
            .map_err(|error| ApiError::bad(&error.to_string()))
    }
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
        let session = self.store.load(id).await.map_err(ApiError::storage)?;
        let thread = self.core_thread(&session).await?;
        let mut model = thread
            .handle
            .model(selection.map(|s| s.profile))
            .await
            .map_err(|error| ApiError::bad(&error.to_string()))?;
        let active = registry
            .active
            .get(id)
            .and_then(|run| registry.runs.get(run))
            .and_then(|entry| entry.info().model);
        model.active = active;
        Ok(model)
    }
}
