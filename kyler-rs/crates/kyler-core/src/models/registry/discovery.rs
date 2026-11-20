//! Host-owned discovery cache and atomic publication; no inference requests.
use super::ModelRegistry;

use std::{collections::BTreeMap, io::Write, sync::Arc};

use anyhow::Result;
use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use kyler_config::{AppConfig, ModelProfile, ModelSettings, ProviderSettings};
use kyler_protocol::api::models::{ModelCatalog, ProviderDiscoveryState};
use kyler_provider::{Adapter, OutputTokenParameter, discovery::DiscoveredModel};

#[derive(Clone, Default, Deserialize, Serialize)]
pub(super) struct Cache(pub BTreeMap<String, Vec<DiscoveredModel>>);
fn identity(name: &str, p: &ProviderSettings) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!(
            "{name}\0{}\0{}\0{:?}",
            p.base_url,
            p.api_key.expose(),
            p.adapter
        ))
    )
}
fn cache_path(config: &AppConfig) -> std::path::PathBuf {
    config.session.directory.join("_model-catalog.json")
}
pub(super) fn read_cache(config: &AppConfig) -> Cache {
    let path = cache_path(config);
    if !config.session.enabled
        || !std::fs::metadata(&path).is_ok_and(|m| m.len() <= 2 * 1024 * 1024)
    {
        return Cache::default();
    }
    std::fs::read(path)
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default()
}
fn write_cache(config: &AppConfig, cache: &Cache) -> Result<()> {
    if !config.session.enabled {
        return Ok(());
    }
    let path = cache_path(config);
    std::fs::create_dir_all(&config.session.directory)?;
    let mut file = tempfile::NamedTempFile::new_in(&config.session.directory)?;
    let bytes = serde_json::to_vec(cache)?;
    anyhow::ensure!(
        bytes.len() <= 2 * 1024 * 1024,
        "Model cache exceeds size limit"
    );
    file.write_all(&bytes)?;
    file.flush()?;
    file.persist(path)?;
    Ok(())
}
pub(super) fn capabilities<'a>(
    cache: &'a Cache,
    name: &str,
    provider: &ProviderSettings,
    model: &str,
) -> Option<&'a kyler_protocol::api::models::ModelCapabilities> {
    cache
        .0
        .get(&identity(name, provider))?
        .iter()
        .find(|entry| entry.id == model)
        .map(|entry| &entry.capabilities)
}
pub(super) fn extend_profiles(settings: &mut ModelSettings, config: &AppConfig, cache: &Cache) {
    for (name, provider) in &settings.providers {
        if !provider.discover_models {
            continue;
        }
        let adapter = Adapter::resolve(provider.adapter, &provider.base_url);
        let Some(entries) = cache.0.get(&identity(name, provider)) else {
            continue;
        };
        for entry in entries {
            if entry.id.is_empty()
                || entry.id.len() > 256
                || entry.id.chars().any(char::is_control)
                || !adapter.selectable_model(&entry.id)
                || settings
                    .profiles
                    .values()
                    .any(|p| p.provider == *name && p.model == entry.id)
            {
                continue;
            }
            let key = format!(
                "discovered_{:x}",
                Sha256::digest(format!("{name}\0{}", entry.id))
            );
            settings
                .profiles
                .entry(key)
                .or_insert_with(|| ModelProfile {
                    provider: name.clone(),
                    model: entry.id.clone(),
                    reasoning_effort: None,
                    capabilities: Default::default(),
                    auto_context_window: true,
                    context_tokens: adapter
                        .capabilities(&entry.id, None)
                        .max_context_tokens
                        .unwrap_or(config.context.context_tokens),
                    output_tokens: adapter
                        .capabilities(&entry.id, None)
                        .max_output_tokens
                        .map_or(config.context.output_tokens, |max| {
                            max.min(config.context.output_tokens)
                        }),
                    output_token_parameter: OutputTokenParameter::Auto,
                });
        }
    }
    // A provider-only configuration becomes usable after its first successful discovery.
    if settings.default.is_empty() {
        settings.default = settings.profiles.keys().next().cloned().unwrap_or_default();
    }
}
impl ModelRegistry {
    /// Setup reuses the host-owned cache without constructing a runtime or sending requests.
    pub fn cached_model_ids(
        config: &AppConfig,
        name: &str,
        provider: &ProviderSettings,
    ) -> Vec<DiscoveredModel> {
        let adapter = Adapter::resolve(provider.adapter, &provider.base_url);
        read_cache(config)
            .0
            .remove(&identity(name, provider))
            .unwrap_or_default()
            .into_iter()
            .filter(|m| {
                !m.id.is_empty()
                    && m.id.len() <= 256
                    && !m.id.chars().any(char::is_control)
                    && adapter.selectable_model(&m.id)
            })
            .collect()
    }
    pub fn cache_model_ids(
        config: &AppConfig,
        name: &str,
        provider: &ProviderSettings,
        models: Vec<DiscoveredModel>,
    ) -> Result<()> {
        let mut cache = read_cache(config);
        cache.0.insert(identity(name, provider), models);
        write_cache(config, &cache)
    }
    /// Best-effort startup refresh. Return immediately; cached/configured models remain usable.
    pub fn start_discovery(self: &Arc<Self>) {
        let registry = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let Some(current) = registry.upgrade() else {
                    break;
                };
                if current.refresh_discovery().await.is_err() {
                    tracing::warn!("Model discovery could not publish its catalog");
                }
                drop(current);
                tokio::time::sleep(std::time::Duration::from_secs(300)).await;
            }
        });
    }
    pub async fn refresh_discovery(&self) -> Result<ModelCatalog> {
        let _refresh = self.refresh.lock().await;
        let (config, revision, mut cache) = {
            let current = self
                .current
                .read()
                .map_err(|_| anyhow::anyhow!("model registry unavailable"))?;
            (
                current.config.clone(),
                current.revision,
                current.discovered.clone(),
            )
        };
        let providers = config.effective_models().providers;
        if !providers.values().any(|p| p.discover_models) {
            return Ok(self.catalog());
        }
        let results = stream::iter(providers.into_iter().map(|(name, p)| async move {
            let result = if p.discover_models {
                Some(
                    kyler_provider::discovery::list_models(
                        &p.base_url,
                        p.api_key.expose(),
                        Adapter::resolve(p.adapter, &p.base_url),
                    )
                    .await,
                )
            } else {
                None
            };
            (name, p, result)
        }))
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
        let mut statuses = Vec::new();
        let mut live = Cache::default();
        for (name, p, result) in results {
            let key = identity(&name, &p);
            let (status, error) = match result {
                Some(Ok(entries)) => {
                    cache.0.insert(key.clone(), entries);
                    ("ready", None)
                }
                Some(Err(e)) => ("stale", Some(e.to_string())),
                None => ("disabled", None),
            };
            if let Some(entries) = cache.0.remove(&key) {
                live.0.insert(key, entries);
            }
            statuses.push(ProviderDiscoveryState {
                provider: name,
                status: status.into(),
                error,
            });
        }
        statuses.sort_by(|a, b| a.provider.cmp(&b.provider));
        let next_revision = revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("model revision exhausted"))?;
        let mut candidate = Self::build_discovered(&config, next_revision, live, statuses)?;
        let mut current = self
            .current
            .write()
            .map_err(|_| anyhow::anyhow!("model registry unavailable"))?;
        // Configuration edits invalidate in-flight refreshes, including their cache writes.
        if current.revision != revision {
            return Ok(Self::describe(&current));
        }
        if candidate.config.effective_models().default.is_empty() && !current.default.is_empty() {
            candidate.default = current.default.clone();
        }
        if write_cache(&config, &candidate.discovered).is_err() {
            tracing::warn!("Model catalog cache could not be saved");
        }
        *current = candidate;
        Ok(Self::describe(&current))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
    use kyler_provider::AdapterKind;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn discovered_models_survive_restart_and_failed_refresh_without_secrets() {
        let root = tempfile::tempdir().unwrap();
        let failed = Arc::new(AtomicBool::new(false));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let router=Router::new().route("/v1/models",get(|State(failed):State<Arc<AtomicBool>>|async move {
            if failed.load(Ordering::SeqCst) {(StatusCode::UNAUTHORIZED,Json(serde_json::json!({"error":"PRIVATE_PROVIDER_BODY"})))}
            else {(StatusCode::OK,Json(serde_json::json!({"data":[{"id":"gpt-5.6-sol"},{"id":"gpt-5.6-sol"},{"id":"text-embedding-3-small"}]})))}
        })).with_state(failed.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut config = AppConfig::default();
        config.session.enabled = true;
        config.session.directory = root.path().join("sessions");
        config.models.providers.insert(
            "fixture".into(),
            ProviderSettings {
                adapter: AdapterKind::Openai,
                base_url: base,
                api_key: kyler_config::SecretString::new("PRIVATE_FIXTURE_KEY"),
                ..Default::default()
            },
        );
        let registry = ModelRegistry::new(&config).unwrap();
        assert!(registry.catalog().models.is_empty());
        let catalog = registry.refresh_discovery().await.unwrap();
        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.providers[0].status, "ready");
        let old = registry.resolve(None).unwrap();
        assert_eq!(old.info.model, "gpt-5.6-sol");
        assert_eq!(
            old.output_token_parameter,
            OutputTokenParameter::MaxCompletionTokens
        );
        assert_eq!(
            old.info
                .capabilities
                .reasoning
                .as_ref()
                .unwrap()
                .levels
                .len(),
            6
        );
        let cache = std::fs::read_to_string(cache_path(&config)).unwrap();
        assert!(!cache.contains("PRIVATE"));
        let restarted = ModelRegistry::new(&config).unwrap();
        assert_eq!(restarted.resolve(None).unwrap().info.model, "gpt-5.6-sol");
        failed.store(true, Ordering::SeqCst);
        let stale = restarted.refresh_discovery().await.unwrap();
        assert_eq!(stale.models.len(), 1);
        assert_eq!(stale.providers[0].status, "stale");
        assert!(!serde_json::to_string(&stale).unwrap().contains("PRIVATE"));
        assert_eq!(old.info.revision, catalog.revision);
        config.models.providers.get_mut("fixture").unwrap().api_key =
            kyler_config::SecretString::new("ROTATED_FIXTURE_KEY");
        assert!(
            ModelRegistry::new(&config)
                .unwrap()
                .catalog()
                .models
                .is_empty()
        );
        server.abort();
    }

    #[tokio::test]
    async fn configuration_reload_discards_inflight_discovery() {
        let root = tempfile::tempdir().unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let state = (started.clone(), release.clone());
        let router = Router::new()
            .route(
                "/v1/models",
                get(
                    |State((started, release)): State<(
                        Arc<tokio::sync::Notify>,
                        Arc<tokio::sync::Notify>,
                    )>| async move {
                        started.notify_one();
                        release.notified().await;
                        Json(serde_json::json!({"data":[{"id":"stale-model"}]}))
                    },
                ),
            )
            .with_state(state);
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut config = AppConfig::default();
        config.session.directory = root.path().join("sessions");
        config.llm.base_url = base;
        config.llm.api_key = kyler_config::SecretString::new("fixture");
        config.llm.model = "original".into();
        let registry = Arc::new(ModelRegistry::new(&config).unwrap());
        let r = registry.clone();
        let refresh = tokio::spawn(async move { r.refresh_discovery().await.unwrap() });
        tokio::time::timeout(std::time::Duration::from_secs(30), started.notified())
            .await
            .unwrap();
        config.llm.model = "replacement".into();
        registry.reload(&config).unwrap();
        release.notify_one();
        let catalog = refresh.await.unwrap();
        assert_eq!(catalog.models.len(), 1);
        assert_eq!(registry.resolve(None).unwrap().info.model, "replacement");
        assert!(!cache_path(&config).exists());
        server.abort();
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;
    use kyler_protocol::api::models::{ModelCapabilities, ReasoningCapabilities, ThinkingLevel};
    #[test]
    fn discovered_capabilities_survive_restart_and_apply_to_existing_profiles() {
        let root = tempfile::tempdir().unwrap();
        let mut config = AppConfig::default();
        config.session.enabled = true;
        config.session.directory = root.path().join("sessions");
        config.llm.base_url = "https://custom.example/v1".into();
        config.llm.model = "custom-reasoning".into();
        config.llm.api_key = kyler_config::SecretString::new("PRIVATE_FIXTURE_KEY");
        let settings = config.effective_models();
        let (name, provider) = settings.providers.iter().next().unwrap();
        ModelRegistry::cache_model_ids(
            &config,
            name,
            provider,
            vec![DiscoveredModel {
                id: config.llm.model.clone(),
                capabilities: ModelCapabilities {
                    reasoning: Some(ReasoningCapabilities {
                        levels: vec![ThinkingLevel::High, ThinkingLevel::Ultra],
                        default: ThinkingLevel::High,
                        source: "provider".into(),
                    }),
                    ..Default::default()
                },
            }],
        )
        .unwrap();
        let registry = ModelRegistry::new(&config).unwrap();
        let bound = registry.resolve(None).unwrap();
        assert_eq!(
            bound.info.capabilities.reasoning.as_ref().unwrap().levels,
            vec![ThinkingLevel::High, ThinkingLevel::Ultra]
        );
        assert_eq!(bound.reasoning_effort, Some(ThinkingLevel::High));
        let key = bound.preference_key();
        config.llm.base_url = "https://other.example/v1".into();
        let replacement = ModelRegistry::new(&config).unwrap().resolve(None).unwrap();
        assert!(replacement.info.capabilities.reasoning.is_none());
        assert_ne!(key, replacement.preference_key());
        config.llm.base_url = "https://custom.example/v1".into();
        config.llm.api_key = kyler_config::SecretString::new("ROTATED_FIXTURE_KEY");
        assert_eq!(
            key,
            ModelRegistry::new(&config)
                .unwrap()
                .resolve(None)
                .unwrap()
                .preference_key()
        );
        let cache = std::fs::read_to_string(cache_path(&config)).unwrap();
        assert!(!cache.contains("PRIVATE_FIXTURE_KEY"));
    }
}
