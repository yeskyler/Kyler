//! Credential-safe model edits through the trusted configuration source.
use crate::{
    error::{ApiError, ApiResult, valid_id},
    runs::RunManager,
};

use std::{fs, io::Write};

use anyhow::{Context, Result, ensure};

use kyler_config::{SecretString, loader, models::validate_provider};
use kyler_protocol::api::models::{ModelConnectionSettings, ModelConnectionUpdate};

impl RunManager {
    pub async fn model_connection(&self, id: &str) -> ApiResult<ModelConnectionSettings> {
        self.connection_settings(Some(id)).await
    }
    pub async fn global_model_connection(&self) -> ApiResult<ModelConnectionSettings> {
        self.connection_settings(None).await
    }
    async fn connection_settings(&self, id: Option<&str>) -> ApiResult<ModelConnectionSettings> {
        let selection = if let Some(id) = id {
            valid_id(id)?;
            self.store.load(id).await.map_err(ApiError::storage)?;
            self.read_model_selection(id).await?
        } else {
            Default::default()
        };
        let models = self
            .agent
            .model_registry()
            .ok_or_else(|| ApiError::bad("Model management is unavailable."))?;
        models
            .resolve(selection.profile.as_deref())
            .map(|snapshot| snapshot.connection_settings())
            .map_err(|_| ApiError::bad("The selected model profile is unavailable."))
    }

    pub async fn update_model_connection(
        &self,
        id: &str,
        request: ModelConnectionUpdate,
    ) -> ApiResult<ModelConnectionSettings> {
        self.update_connection_settings(Some(id), request).await
    }
    pub async fn update_global_model_connection(
        &self,
        request: ModelConnectionUpdate,
    ) -> ApiResult<ModelConnectionSettings> {
        self.update_connection_settings(None, request).await
    }
    async fn update_connection_settings(
        &self,
        id: Option<&str>,
        request: ModelConnectionUpdate,
    ) -> ApiResult<ModelConnectionSettings> {
        let _reload = self.model_reload.lock().await;
        let _admission = self.registry.lock().await;
        let selection = if let Some(id) = id {
            valid_id(id)?;
            self.store.load(id).await.map_err(ApiError::storage)?;
            self.read_model_selection(id).await?
        } else {
            Default::default()
        };
        let models = self
            .agent
            .model_registry()
            .cloned()
            .ok_or_else(|| ApiError::bad("Model management is unavailable."))?;
        let snapshot = models
            .resolve(selection.profile.as_deref())
            .map_err(|_| ApiError::bad("The selected model profile is unavailable."))?;
        let source = self
            .model_source
            .read()
            .map_err(|_| ApiError::internal())?
            .clone()
            .ok_or_else(|| {
                ApiError::bad(
                    "This server has no editable configuration source. Start it with kyler serve.",
                )
            })?;
        let mut provider = snapshot.provider_settings();
        provider.base_url = request
            .url
            .trim()
            .trim_end_matches('/')
            .trim_end_matches("/chat/completions")
            .trim_end_matches('/')
            .to_owned();
        let model = request.model.trim().to_owned();
        if model.is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
            return Err(ApiError::bad(
                "Model must contain 1 to 256 characters without control characters.",
            ));
        }
        if let Some(key) = request.key.filter(|key| !key.trim().is_empty()) {
            let key = key.trim();
            if key.len() > 4096 || !key.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(ApiError::bad(
                    "Key must contain visible ASCII characters without spaces.",
                ));
            }
            provider.api_key = SecretString::new(key);
        }
        validate_provider(&provider).map_err(|_| ApiError::bad("URL must be HTTP(S) without credentials, query or fragment, and a key must be configured."))?;
        let profile = snapshot.info.profile.clone();
        let previous_model = snapshot.info.model.clone();
        tokio::task::spawn_blocking(move || -> Result<ModelConnectionSettings> {
            let path = fs::canonicalize(&source.0).context("Cannot locate the Kyler configuration file.")?;
            let text = fs::read_to_string(&path).context("Cannot read the Kyler configuration file.")?;
            let mut values: toml::Value = toml::from_str(&text).map_err(|_| anyhow::anyhow!("The Kyler configuration file is invalid."))?;
            let raw = loader::parse(&text)?;
            let root = values.as_table_mut().context("Configuration must be a TOML table.")?;
            if raw.models.profiles.is_empty() {
                let llm = table(root, "llm")?;
                llm.insert("base_url".into(), provider.base_url.clone().into());
                llm.insert("model".into(), model.clone().into());
                llm.insert("api_key".into(), provider.api_key.expose().into());
                if previous_model != model { llm.remove("capabilities"); }
            } else {
                let selected = raw.models.profiles.get(&profile).context("The selected profile changed. Reopen model settings.")?;
                let shared = raw.models.profiles.values().filter(|p| p.provider == selected.provider).count() > 1;
                // Editing one model must not redirect another profile's provider.
                let provider_name = if shared { format!("web_{}", uuid::Uuid::new_v4().simple()) } else { selected.provider.clone() };
                let settings = table(root, "models")?;
                let profile_table = table(table(settings, "profiles")?, &profile)?;
                profile_table.insert("model".into(), model.clone().into());
                profile_table.insert("provider".into(), provider_name.clone().into());
                if previous_model != model { profile_table.remove("capabilities"); }
                let connection = table(table(settings, "providers")?, &provider_name)?;
                connection.insert("base_url".into(), provider.base_url.clone().into());
                connection.insert("api_key".into(), provider.api_key.expose().into());
                connection.remove("api_key_env");
            }
            let encoded = toml::to_string_pretty(&values).map_err(|_| anyhow::anyhow!("Cannot encode the model settings."))?;
            let directory = path.parent().context("Configuration directory is unavailable.")?;
            let mut temporary = tempfile::NamedTempFile::new_in(directory).context("Cannot stage the model settings.")?;
            temporary.write_all(encoded.as_bytes()).context("Cannot write the model settings.")?;
            temporary.as_file().sync_all()?;
            // Same parent directory preserves all relative configuration paths.
            let candidate = loader::load_with_llm_overrides(Some(temporary.path()), &source.1)?;
            let effective = candidate.effective_models();
            let selected = effective.profiles.get(&profile).context("The selected profile is unavailable.")?;
            let effective_provider = &effective.providers[&selected.provider];
            ensure!(selected.model == model && effective_provider.base_url == provider.base_url && effective_provider.api_key.expose() == provider.api_key.expose(),
                "A startup environment or command-line override controls these model fields. Remove that override before editing them here.");
            models.reload_after(&candidate, || {
                ensure!(fs::read_to_string(&path)? == text, "Configuration changed while saving. Reopen model settings and retry.");
                temporary.persist(&path).map_err(|_| anyhow::anyhow!("Cannot save the model settings."))?;
                fs::File::open(directory)?.sync_all()?;
                Ok(())
            })?;
            Ok(ModelConnectionSettings { url: provider.base_url, model, key_configured: true })
        }).await.map_err(|_| ApiError::internal())?.map_err(|error| ApiError::bad(&error.to_string()))
    }
}

fn table<'a>(root: &'a mut toml::Table, key: &str) -> Result<&'a mut toml::Table> {
    root.entry(key)
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .context("Expected a configuration table.")
}
