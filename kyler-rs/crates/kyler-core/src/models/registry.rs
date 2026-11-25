//! Atomic model catalog replacement and immutable execution snapshots.
mod discovery;

use super::snapshot::ModelSnapshot;
use crate::context::ContextPolicy;

use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use anyhow::{Result, ensure};

use kyler_config::AppConfig;
use kyler_protocol::api::models::{ModelCatalog, ModelInfo};

struct Catalog {
    config: AppConfig,
    discovered: discovery::Cache,
    statuses: Vec<kyler_protocol::api::models::ProviderDiscoveryState>,
    revision: u64,
    default: String,
    models: BTreeMap<String, Arc<ModelSnapshot>>,
}
pub struct ModelRegistry {
    current: RwLock<Catalog>,
    refresh: tokio::sync::Mutex<()>,
}
impl ModelRegistry {
    pub fn new(config: &AppConfig) -> Result<Self> {
        Ok(Self {
            current: RwLock::new(Self::build(config, 1)?),
            refresh: tokio::sync::Mutex::new(()),
        })
    }
    fn build(config: &AppConfig, revision: u64) -> Result<Catalog> {
        Self::build_discovered(config, revision, discovery::read_cache(config), Vec::new())
    }
    fn build_discovered(
        config: &AppConfig,
        revision: u64,
        discovered: discovery::Cache,
        mut statuses: Vec<kyler_protocol::api::models::ProviderDiscoveryState>,
    ) -> Result<Catalog> {
        let mut settings = config.effective_models();
        discovery::extend_profiles(&mut settings, config, &discovered);
        settings.validate()?;
        if statuses.is_empty() {
            statuses = settings
                .providers
                .iter()
                .map(
                    |(name, p)| kyler_protocol::api::models::ProviderDiscoveryState {
                        provider: name.clone(),
                        display_name: String::new(),
                        adapter: String::new(),
                        checked_at: None,
                        connection_status: "untested".into(),
                        status: if p.discover_models {
                            "pending"
                        } else {
                            "disabled"
                        }
                        .into(),
                        error: None,
                    },
                )
                .collect();
        }
        for status in &mut statuses {
            if let Some(provider) = settings.providers.get(&status.provider) {
                let adapter =
                    kyler_provider::Adapter::resolve(provider.adapter, &provider.base_url);
                let name = adapter.0.preset().name;
                status.display_name = if status.provider == "default" {
                    name.into()
                } else {
                    format!("{name} ({})", status.provider)
                };
                status.adapter = serde_json::to_value(adapter.0)?
                    .as_str()
                    .unwrap()
                    .to_owned();
            }
        }
        let mut models = BTreeMap::new();
        for (name, profile) in settings.profiles {
            let provider = &settings.providers[&profile.provider];
            if provider
                .enabled_models
                .as_ref()
                .is_some_and(|models| !models.contains(&profile.model))
            {
                continue;
            }
            let adapter = kyler_provider::Adapter::resolve(provider.adapter, &provider.base_url);
            // Old connection edits may have left hosted aliases in persisted profiles.
            if !adapter.accepts_model_id(&profile.model) {
                continue;
            }
            let capabilities = adapter.resolve_capabilities(
                &profile.model,
                &profile.capabilities,
                discovery::capabilities(&discovered, &profile.provider, provider, &profile.model),
                profile.reasoning_effort,
            )?;
            let reasoning_effort = profile.reasoning_effort.or_else(|| {
                capabilities
                    .reasoning
                    .as_ref()
                    .filter(|caps| !caps.levels.is_empty())
                    .map(|caps| caps.default)
            });
            let mut policy = ContextPolicy::from(&config.context);
            policy.context_tokens = if profile.auto_context_window {
                capabilities
                    .max_context_tokens
                    .unwrap_or(profile.context_tokens)
            } else {
                profile.context_tokens
            };
            policy.output_tokens = profile.output_tokens;
            policy.validate()?;
            models.insert(
                name.clone(),
                Arc::new(ModelSnapshot {
                    adapter,
                    adapter_kind: provider.adapter,
                    protocol_kind: provider.protocol,
                    discover_models: provider.discover_models,
                    info: ModelInfo {
                        protocol: adapter.protocol(&profile.model, provider.protocol),
                        capabilities,
                        profile: name,
                        provider: profile.provider,
                        model: profile.model.clone(),
                        revision,
                        context_window_tokens: policy.context_tokens,
                        sliding_window_tokens: policy.sliding_window_tokens(),
                        output_tokens: policy.output_tokens,
                    },
                    policy,
                    output_token_parameter: adapter
                        .output_parameter(&profile.model, profile.output_token_parameter),
                    reasoning_effort,
                    manual_reasoning: false,
                    base_url: provider.base_url.clone(),
                    api_key: provider.api_key.clone(),
                }),
            );
        }
        Ok(Catalog {
            config: config.clone(),
            discovered,
            statuses,
            revision,
            default: settings.default,
            models,
        })
    }
    pub fn reload(&self, config: &AppConfig) -> Result<ModelCatalog> {
        self.reload_after(config, || Ok(()))
    }
    /// Validate the next generation before publishing its configuration. A failed
    /// publish leaves the active catalog and all execution snapshots unchanged.
    pub fn reload_after(
        &self,
        config: &AppConfig,
        publish: impl FnOnce() -> Result<()>,
    ) -> Result<ModelCatalog> {
        // Build before taking the write lock; readers keep the last valid generation.
        let mut discovered = discovery::read_cache(config);
        {
            let current = self
                .current
                .read()
                .map_err(|_| anyhow::anyhow!("model registry unavailable"))?;
            discovered.0.extend(current.discovered.0.clone());
        }
        let mut candidate = Self::build_discovered(config, 0, discovered, Vec::new())?;
        let mut current = self
            .current
            .write()
            .map_err(|_| anyhow::anyhow!("model registry unavailable"))?;
        candidate.revision = current
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("model revision exhausted"))?;
        for model in candidate.models.values_mut() {
            Arc::make_mut(model).info.revision = candidate.revision;
        }
        publish()?;
        *current = candidate;
        Ok(Self::describe(&current))
    }
    fn describe(catalog: &Catalog) -> ModelCatalog {
        ModelCatalog {
            revision: catalog.revision,
            default_profile: catalog.default.clone(),
            models: catalog.models.values().map(|m| m.info.clone()).collect(),
            providers: catalog.statuses.clone(),
        }
    }
    pub fn catalog(&self) -> ModelCatalog {
        Self::describe(&self.current.read().expect("model registry poisoned"))
    }
    pub fn resolve(&self, profile: Option<&str>) -> Result<Arc<ModelSnapshot>> {
        let catalog = self
            .current
            .read()
            .map_err(|_| anyhow::anyhow!("model registry unavailable"))?;
        let name = profile.unwrap_or(&catalog.default);
        ensure!(
            catalog.models.contains_key(name),
            "selected model profile '{name}' is unavailable; select another model"
        );
        Ok(catalog.models[name].clone())
    }
}
#[cfg(test)]
mod tests {
    use super::ModelRegistry;
    use crate::models::set_output_limit;

    use kyler_config::{AppConfig, OutputTokenParameter};

    fn config() -> AppConfig {
        kyler_config::loader::parse(
            r#"
[models]
default = "fast"
[models.providers.fixture]
base_url = "http://localhost:1234/v1"
api_key = "PRIVATE_FIXTURE_SECRET"
[models.profiles.fast]
provider = "fixture"
model = "first"
context_tokens = 64000
output_tokens = 2048
"#,
        )
        .unwrap()
    }
    #[test]
    fn reload_is_atomic_and_snapshots_outlive_replacement() {
        let mut config = config();
        let registry = ModelRegistry::new(&config).unwrap();
        let old = registry.resolve(None).unwrap();
        config
            .models
            .profiles
            .get_mut("fast")
            .unwrap()
            .context_tokens = 100;
        assert!(registry.reload(&config).is_err());
        assert_eq!(registry.catalog().revision, 1);
        config
            .models
            .profiles
            .get_mut("fast")
            .unwrap()
            .context_tokens = 32000;
        config.models.profiles.get_mut("fast").unwrap().model = "second".into();
        assert_eq!(registry.reload(&config).unwrap().revision, 2);
        assert_eq!(old.info.model, "first");
        assert_eq!(old.info.context_window_tokens, 64000);
        let new = registry.resolve(None).unwrap();
        assert_eq!(new.info.model, "second");
        assert_eq!(new.info.sliding_window_tokens, 19200);
        assert!(registry.resolve(Some("removed")).is_err());
        assert!(
            !serde_json::to_string(&registry.catalog())
                .unwrap()
                .contains("PRIVATE_FIXTURE_SECRET")
        );
        assert!(
            !serde_json::to_string(&registry.catalog())
                .unwrap()
                .contains("base_url")
        );
    }
    #[test]
    fn failed_configuration_publish_keeps_previous_generation() {
        let mut config = config();
        let registry = ModelRegistry::new(&config).unwrap();
        config.models.profiles.get_mut("fast").unwrap().model = "replacement".into();
        assert!(
            registry
                .reload_after(&config, || anyhow::bail!("write failed"))
                .is_err()
        );
        assert_eq!(registry.catalog().revision, 1);
        assert_eq!(registry.resolve(None).unwrap().info.model, "first");
    }
    #[test]
    fn requests_use_exactly_one_output_limit_parameter() {
        use async_openai::types::chat::{
            ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
        };

        let mut request = CreateChatCompletionRequestArgs::default()
            .model("fixture")
            .messages(vec![
                ChatCompletionRequestUserMessageArgs::default()
                    .content("hello")
                    .build()
                    .unwrap()
                    .into(),
            ])
            .build()
            .unwrap();
        set_output_limit(&mut request, OutputTokenParameter::MaxCompletionTokens, 123);
        let modern = serde_json::to_value(&request).unwrap();
        assert_eq!(modern["max_completion_tokens"], 123);
        assert!(modern.get("max_tokens").is_none());
        set_output_limit(&mut request, OutputTokenParameter::MaxTokens, 456);
        let compatible = serde_json::to_value(&request).unwrap();
        assert_eq!(compatible["max_tokens"], 456);
        assert!(compatible.get("max_completion_tokens").is_none());
    }
}

#[cfg(test)]
mod capability_tests {
    use super::ModelRegistry;

    #[test]
    fn automatic_windows_follow_known_model_limits_and_preserve_manual_budgets() {
        let mut config = kyler_config::loader::parse(
            r#"[llm]
base_url="https://api.openai.com/v1"
model="gpt-5.6-sol"
api_key="fixture"
[context]
context_tokens=64000
"#,
        )
        .unwrap();
        assert_eq!(
            ModelRegistry::new(&config)
                .unwrap()
                .resolve(None)
                .unwrap()
                .policy
                .context_tokens,
            64000
        );
        config.context.auto_model_window = true;
        assert_eq!(
            ModelRegistry::new(&config)
                .unwrap()
                .resolve(None)
                .unwrap()
                .policy
                .context_tokens,
            1_050_000
        );
        config.llm.model = "unknown".into();
        assert_eq!(
            ModelRegistry::new(&config)
                .unwrap()
                .resolve(None)
                .unwrap()
                .policy
                .context_tokens,
            64000
        );
    }
    #[test]
    fn legacy_profiles_preserve_explicit_capabilities_and_tokenizer() {
        let config = kyler_config::loader::parse(
            r#"
[llm]
base_url="http://127.0.0.1:1/v1"
api_key="fixture"
model="text-model"
[llm.capabilities]
images=false
tools=true
structured_output=false
tokenizer="cl100k_base"
"#,
        )
        .unwrap();
        let registry = ModelRegistry::new(&config).unwrap();
        let model = registry.resolve(None).unwrap();
        assert_eq!(model.info.capabilities.images, Some(false));
        assert_eq!(model.info.capabilities.tools, Some(true));
        assert_eq!(
            model.info.capabilities.tokenizer,
            kyler_protocol::api::models::Tokenizer::Cl100kBase
        );
    }
}

#[cfg(test)]
mod thinking_tests {
    use super::ModelRegistry;
    #[test]
    fn legacy_and_named_profile_thinking_settings_bind_to_the_model_generation() {
        let mut config = kyler_config::loader::parse(
            r#"
[llm]
base_url="https://api.siliconflow.cn/v1"
model="deepseek-ai/DeepSeek-V4-Flash"
api_key="fixture"
"#,
        )
        .unwrap();
        let registry = ModelRegistry::new(&config).unwrap();
        let old = registry.resolve(None).unwrap();
        assert_eq!(
            old.reasoning_effort,
            Some(kyler_protocol::api::models::ThinkingLevel::High)
        );
        config.llm.reasoning_effort = Some(kyler_protocol::api::models::ThinkingLevel::Off);
        registry.reload(&config).unwrap();
        assert_eq!(
            registry.resolve(None).unwrap().reasoning_effort,
            Some(kyler_protocol::api::models::ThinkingLevel::Off)
        );
        assert_eq!(
            old.reasoning_effort,
            Some(kyler_protocol::api::models::ThinkingLevel::High)
        );
        config.models = config.effective_models();
        config
            .models
            .profiles
            .get_mut("default")
            .unwrap()
            .reasoning_effort = Some(kyler_protocol::api::models::ThinkingLevel::High);
        assert_eq!(
            ModelRegistry::new(&config)
                .unwrap()
                .resolve(None)
                .unwrap()
                .reasoning_effort,
            Some(kyler_protocol::api::models::ThinkingLevel::High)
        );
    }
}

#[cfg(test)]
mod provider_label_tests {
    use super::*;

    #[test]
    fn native_deepseek_catalog_excludes_persisted_hosted_aliases() {
        let mut config = AppConfig::default();
        config.llm.base_url = "https://api.deepseek.com/v1".into();
        config.llm.model = "deepseek-flash".into();
        config.llm.api_key = kyler_config::SecretString::new("TEST_ONLY_KEY");
        config.models = config.effective_models();
        let mut legacy = config.models.profiles["default"].clone();
        legacy.model = "Qwen/Qwen3-32B".into();
        config
            .models
            .profiles
            .insert("old-hosted".into(), legacy.clone());
        legacy.model = "deepseek-ai/DeepSeek-V4-Flash".into();
        config
            .models
            .profiles
            .insert("old-deepseek-alias".into(), legacy);
        let catalog = ModelRegistry::new(&config).unwrap().catalog();
        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.models[0].model, "deepseek-flash");
        // Compatible hosting connections continue to accept qualified model IDs.
        config.models.providers.get_mut("default").unwrap().adapter =
            kyler_provider::AdapterKind::Siliconflow;
        assert_eq!(
            ModelRegistry::new(&config).unwrap().catalog().models.len(),
            3
        );
    }

    #[test]
    fn provider_labels_use_connection_identity_and_never_credentials() {
        let mut config = AppConfig::default();
        config.llm.base_url = "https://api.deepseek.com/v1".into();
        config.llm.model = "deepseek-flash".into();
        config.llm.api_key = kyler_config::SecretString::new("PRIVATE_LABEL_TEST_KEY");
        let catalog = ModelRegistry::new(&config).unwrap().catalog();
        assert_eq!(catalog.providers[0].provider, "default");
        assert_eq!(catalog.providers[0].display_name, "DeepSeek");
        assert_eq!(catalog.providers[0].adapter, "deepseek");
        assert!(
            !serde_json::to_string(&catalog)
                .unwrap()
                .contains("PRIVATE_LABEL_TEST_KEY")
        );

        config.models = config.effective_models();
        let provider = config.models.providers.remove("default").unwrap();
        let profile = config.models.profiles.remove("default").unwrap();
        for name in ["work", "personal"] {
            config
                .models
                .providers
                .insert(name.into(), provider.clone());
            let mut profile = profile.clone();
            profile.provider = name.into();
            config.models.profiles.insert(name.into(), profile);
        }
        config.models.default = "work".into();
        let catalog = ModelRegistry::new(&config).unwrap().catalog();
        assert_eq!(catalog.providers.len(), 2);
        assert!(
            catalog
                .providers
                .iter()
                .any(|p| p.provider == "work" && p.display_name == "DeepSeek (work)")
        );
        assert!(
            catalog
                .providers
                .iter()
                .any(|p| p.provider == "personal" && p.display_name == "DeepSeek (personal)")
        );
        assert_eq!(
            catalog
                .models
                .iter()
                .filter(|m| m.model == "deepseek-flash")
                .count(),
            2
        );
        assert_eq!(
            catalog
                .models
                .iter()
                .find(|m| m.profile == "work")
                .unwrap()
                .provider,
            "work"
        );
    }
}
