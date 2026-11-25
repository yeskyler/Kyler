//! Trusted-source provider edits with atomic publication and credential-free responses.
use crate::{
    error::{ApiError, ApiResult},
    runs::RunManager,
};

use std::{fs, io::Write};

use anyhow::{Context, Result, ensure};

use kyler_config::{ProviderSettings, SecretString, loader, models::validate_provider};
use kyler_protocol::api::providers::{
    ProviderCatalog, ProviderInfo, ProviderPreset, ProviderTest, ProviderUpdate,
};
use kyler_provider::{Adapter, AdapterKind, PROVIDER_PRESETS};

fn connection_changed(old: &ProviderSettings, new: &ProviderSettings) -> bool {
    old.base_url.trim_end_matches('/') != new.base_url.trim_end_matches('/')
        || Adapter::resolve(old.adapter, &old.base_url).0
            != Adapter::resolve(new.adapter, &new.base_url).0
}
fn checked_provider(
    request: &ProviderUpdate,
    previous: Option<&ProviderSettings>,
) -> Result<ProviderSettings> {
    let adapter: AdapterKind = serde_json::from_value(request.adapter.clone().into())?;
    let mut provider = previous.cloned().unwrap_or_default();
    provider.adapter = adapter;
    provider.base_url = request.base_url.trim().trim_end_matches('/').to_owned();
    if previous.is_some_and(|old| connection_changed(old, &provider)) {
        provider.api_key = SecretString::default();
        provider.api_key_env = None;
        provider.protocol = Default::default();
    }
    if let Some(key) = request.key.as_ref().filter(|key| !key.is_empty()) {
        ensure!(
            key.len() <= 4096 && key.bytes().all(|b| b.is_ascii_graphic()),
            "Invalid key"
        );
        provider.api_key = SecretString::new(key.clone());
        provider.api_key_env = None;
    }
    provider.enabled_models = Some(request.models.clone());
    provider.default_model = request
        .default_model
        .clone()
        .or_else(|| {
            provider
                .default_model
                .clone()
                .filter(|m| request.models.contains(m))
        })
        .or_else(|| request.models.first().cloned());
    ensure!(
        provider
            .default_model
            .as_ref()
            .is_none_or(|m| request.models.contains(m)),
        "Default model must be enabled"
    );
    ensure!(
        request.models.iter().all(
            |model| Adapter::resolve(provider.adapter, &provider.base_url).accepts_model_id(model)
        ),
        "Model ID does not belong to this provider"
    );
    validate_provider(&provider)?;
    ensure!(request.models.len() <= 1000, "Too many model IDs");
    ensure!(
        request
            .models
            .iter()
            .all(|m| !m.trim().is_empty() && m.len() <= 256 && !m.chars().any(char::is_control)),
        "Invalid model ID"
    );
    Ok(provider)
}
fn connection_table(provider: &ProviderSettings) -> Result<toml::Value> {
    let mut value = toml::Table::new();
    value.insert(
        "adapter".into(),
        serde_json::to_value(provider.adapter)?
            .as_str()
            .context("Invalid adapter")?
            .into(),
    );
    value.insert(
        "protocol".into(),
        serde_json::to_value(provider.protocol)?
            .as_str()
            .context("Invalid protocol")?
            .into(),
    );
    value.insert("base_url".into(), provider.base_url.clone().into());
    value.insert("discover_models".into(), provider.discover_models.into());
    if let Some(models) = &provider.enabled_models {
        value.insert("enabled_models".into(), models.clone().into());
    }
    if let Some(model) = &provider.default_model {
        value.insert("default_model".into(), model.clone().into());
    }
    if let Some(env) = &provider.api_key_env {
        value.insert("api_key_env".into(), env.clone().into());
    } else {
        value.insert("api_key".into(), provider.api_key.expose().into());
    }
    Ok(value.into())
}
fn profile_table(profile: &kyler_config::ModelProfile) -> Result<toml::Value> {
    let mut value = toml::Table::new();
    value.insert("provider".into(), profile.provider.clone().into());
    value.insert("model".into(), profile.model.clone().into());
    value.insert(
        "context_tokens".into(),
        (profile.context_tokens as i64).into(),
    );
    value.insert(
        "output_tokens".into(),
        (profile.output_tokens as i64).into(),
    );
    value.insert(
        "auto_context_window".into(),
        profile.auto_context_window.into(),
    );
    value.insert(
        "capabilities".into(),
        toml::Value::try_from(&profile.capabilities)?,
    );
    value.insert(
        "output_token_parameter".into(),
        toml::Value::try_from(profile.output_token_parameter)?,
    );
    if let Some(level) = profile.reasoning_effort {
        value.insert("reasoning_effort".into(), toml::Value::try_from(level)?);
    }
    Ok(value.into())
}
impl RunManager {
    async fn provider_config(&self) -> ApiResult<kyler_config::AppConfig> {
        let source = self
            .model_source
            .read()
            .map_err(|_| ApiError::internal())?
            .clone()
            .ok_or_else(|| ApiError::bad("This server has no editable configuration source."))?;
        tokio::task::spawn_blocking(move || {
            loader::load_with_llm_overrides(Some(&source.0), &source.1)
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::bad("Cannot load provider configuration."))
    }
    pub async fn providers(&self) -> ApiResult<ProviderCatalog> {
        let config = self.provider_config().await?;
        let effective = config.effective_models();
        let catalog = self.model_catalog()?;
        let providers = effective
            .providers
            .iter()
            .map(|(id, provider)| {
                let mut models: Vec<_> = catalog
                    .models
                    .iter()
                    .filter(|m| m.provider == *id)
                    .map(|m| m.model.clone())
                    .collect();
                models.sort();
                models.dedup();
                ProviderInfo {
                    id: id.clone(),
                    adapter: serde_json::to_value(
                        Adapter::resolve(provider.adapter, &provider.base_url).0,
                    )
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .into(),
                    base_url: provider.base_url.clone(),
                    key_configured: !provider.api_key.is_empty(),
                    available_models: self
                        .agent
                        .model_registry()
                        .map(|registry| {
                            registry
                                .discovered_model_ids(id)
                                .into_iter()
                                .map(|m| m.id)
                                .collect()
                        })
                        .unwrap_or_default(),
                    checked_at: catalog
                        .providers
                        .iter()
                        .find(|p| p.provider == *id)
                        .and_then(|p| p.checked_at),
                    is_default: catalog
                        .models
                        .iter()
                        .any(|m| m.provider == *id && m.profile == catalog.default_profile),
                    models: models.clone(),
                    default_model: catalog
                        .models
                        .iter()
                        .find(|m| m.provider == *id && m.profile == catalog.default_profile)
                        .map(|m| m.model.clone())
                        .or_else(|| provider.default_model.clone())
                        .or_else(|| models.first().cloned()),
                    status: catalog
                        .providers
                        .iter()
                        .find(|p| p.provider == *id)
                        .map(|p| p.connection_status.clone())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "untested".into()),
                    discovery_status: catalog
                        .providers
                        .iter()
                        .find(|p| p.provider == *id)
                        .map(|p| p.status.clone())
                        .unwrap_or_else(|| "pending".into()),
                    discovery_error: catalog
                        .providers
                        .iter()
                        .find(|p| p.provider == *id)
                        .and_then(|p| p.error.clone()),
                }
            })
            .collect();
        Ok(ProviderCatalog {
            providers,
            presets: PROVIDER_PRESETS
                .iter()
                .map(|p| ProviderPreset {
                    adapter: serde_json::to_value(p.kind)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .into(),
                    name: p.name.into(),
                    base_url: p.base_url.map(Into::into),
                    key_required: !p.kind.is_local(),
                })
                .collect(),
        })
    }
    pub async fn test_provider(
        &self,
        id: &str,
        request: ProviderUpdate,
    ) -> ApiResult<ProviderTest> {
        ensure_id(id)?;
        let config = self.provider_config().await?;
        let effective = config.effective_models();
        let provider = checked_provider(&request, effective.providers.get(id))
            .map_err(|_| ApiError::bad("Check the provider type, URL, key and model IDs."))?;
        let models = kyler_provider::discovery::list_models(
            &provider.base_url,
            provider.api_key.expose(),
            Adapter::resolve(provider.adapter, &provider.base_url),
        )
        .await
        .map_err(|error| ApiError::bad(&format!("Model list refresh failed: {error}")))?;
        Ok(ProviderTest {
            models: models.into_iter().map(|m| m.id).collect(),
        })
    }
    pub async fn save_provider(
        &self,
        id: String,
        request: Option<ProviderUpdate>,
    ) -> ApiResult<ProviderCatalog> {
        ensure_id(&id)?;
        let _reload = self.model_reload.lock().await;
        let _admission = self.registry.lock().await;
        let source = self
            .model_source
            .read()
            .map_err(|_| ApiError::internal())?
            .clone()
            .ok_or_else(|| ApiError::bad("This server has no editable configuration source."))?;
        let registry = self
            .agent
            .model_registry()
            .cloned()
            .ok_or_else(|| ApiError::bad("Model management is unavailable."))?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let path = fs::canonicalize(&source.0)?;
            let text = fs::read_to_string(&path)?;
            let config = loader::load_with_llm_overrides(Some(&path), &source.1)?;
            let effective = config.effective_models();
            let mut root: toml::Table = toml::from_str(&text)?;
            let mut connections = toml::Table::new();
            for (name, provider) in &effective.providers { connections.insert(name.clone(), connection_table(provider)?); }
            let mut profiles = toml::Table::new();
            for (name, profile) in &effective.profiles { profiles.insert(name.clone(), profile_table(profile)?); }
            let mut default = effective.default.clone();
            if let Some(request) = request {
                let provider = checked_provider(&request, effective.providers.get(&id))?;
                connections.insert(id.clone(), connection_table(&provider)?);
                let changed = effective.providers.get(&id).is_some_and(|old| connection_changed(old, &provider));
                profiles.retain(|_, p| p["provider"].as_str() != Some(&id) || (!changed && request.models.iter().any(|m| Some(m.as_str()) == p["model"].as_str())));
                let desired = provider.default_model.clone();
                let mut first = None;
                let mut preferred = None;
                for model in &request.models {
                    let existing = profiles.iter().find(|(_, p)| p["provider"].as_str() == Some(&id) && p["model"].as_str() == Some(model)).map(|(name, _)| name.clone());
                    let name = existing.unwrap_or_else(|| format!("web_{}", uuid::Uuid::new_v4().simple()));
                    if !profiles.contains_key(&name) {
                        let profile = kyler_config::ModelProfile {
                            provider: id.clone(), model: model.clone(), capabilities: Default::default(), reasoning_effort: None,
                            auto_context_window: true, context_tokens: config.context.context_tokens, output_tokens: config.context.output_tokens,
                            output_token_parameter: Default::default(),
                        };
                        profiles.insert(name.clone(), profile_table(&profile)?);
                    }
                    if desired.as_ref() == Some(model) { preferred = Some(name.clone()); }
                    first.get_or_insert(name);
                }
                let editing_default = effective.profiles.get(&default).is_some_and(|p| p.provider == id);
                if request.make_default || default.is_empty() || (editing_default && request.default_model.is_some()) {
                    default = preferred.or(first).context("No default model")?;
                }
            } else {
                ensure!(connections.remove(&id).is_some(), "Provider not found");
                profiles.retain(|_, p| p["provider"].as_str() != Some(&id));
            }
            ensure!(profiles.contains_key(&default), "Choose another default model before deleting the current default provider or model");
            let settings = root.entry("models").or_insert_with(|| toml::Value::Table(Default::default())).as_table_mut().context("Invalid models table")?;
            settings.insert("providers".into(), connections.into()); settings.insert("profiles".into(), profiles.into()); settings.insert("default".into(), default.into());
            let directory = path.parent().context("Configuration directory unavailable")?;
            let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
            temporary.write_all(toml::to_string_pretty(&root)?.as_bytes())?;
            temporary.as_file().sync_all()?;
            let candidate = loader::load_with_llm_overrides(Some(temporary.path()), &source.1)?;
            ensure!(candidate.effective_models().default == root["models"]["default"].as_str().unwrap_or_default(), "Startup overrides control the default model");
            registry.reload_after(&candidate, || {
                ensure!(fs::read_to_string(&path)? == text, "Configuration changed. Reload and retry");
                temporary.persist(&path).map_err(|_| anyhow::anyhow!("Cannot save configuration"))?;
                fs::File::open(directory)?.sync_all()?; Ok(())
            })?;
            Ok(())
        }).await.map_err(|_| ApiError::internal())?.map_err(|_| ApiError::bad("Cannot save provider. Check configuration, model IDs, and ensure the default model is retained."))?;
        drop(_admission);
        if let Some(registry) = self.agent.model_registry() {
            let _ = registry.refresh_discovery().await;
        }
        self.providers().await
    }
}
fn ensure_id(id: &str) -> ApiResult<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(ApiError::bad(
            "Provider name must use 1 to 128 letters, numbers, underscores or hyphens.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_key_is_kept_only_for_the_same_connection() {
        let old = ProviderSettings {
            adapter: AdapterKind::Generic,
            base_url: "http://localhost:1234/v1".into(),
            api_key: SecretString::new("PRIVATE_TEST_KEY"),
            ..Default::default()
        };
        let mut request = ProviderUpdate {
            adapter: "generic".into(),
            base_url: old.base_url.clone(),
            key: None,
            models: vec!["model".into()],
            make_default: false,
            default_model: Some("model".into()),
        };
        assert_eq!(
            checked_provider(&request, Some(&old))
                .unwrap()
                .api_key
                .expose(),
            "PRIVATE_TEST_KEY"
        );
        request.base_url = "http://localhost:5678/v1".into();
        assert!(checked_provider(&request, Some(&old)).is_err());
        request.key = Some("REPLACEMENT_TEST_KEY".into());
        let changed = checked_provider(&request, Some(&old)).unwrap();
        assert_eq!(changed.api_key.expose(), "REPLACEMENT_TEST_KEY");
        request.adapter = "deepseek".into();
        request.models = vec!["Qwen/Qwen3-32B".into()];
        request.default_model = None;
        assert!(checked_provider(&request, Some(&old)).is_err());
        request.models = vec!["deepseek-flash".into()];
        assert!(checked_provider(&request, Some(&old)).is_ok());
        request.default_model = Some("disabled".into());
        assert!(checked_provider(&request, Some(&old)).is_err());
    }
}
