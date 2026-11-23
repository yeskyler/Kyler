//! Configured OpenAI-compatible providers and named model profiles.
use crate::{AppConfig, SecretString};

use std::collections::{BTreeMap, HashMap};

use anyhow::{Result, ensure};
use serde::Deserialize;

use kyler_protocol::api::models::{ModelCapabilities, ThinkingLevel};

pub use kyler_provider::{AdapterKind, OutputTokenParameter, WireProtocol};
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderSettings {
    pub protocol: WireProtocol,
    pub adapter: AdapterKind,
    /// Discover available model IDs without issuing inference requests.
    pub discover_models: bool,
    pub base_url: String,
    pub api_key: SecretString,
    pub api_key_env: Option<String>,
}
impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            protocol: WireProtocol::Auto,
            adapter: AdapterKind::Auto,
            discover_models: true,
            base_url: String::new(),
            api_key: SecretString::default(),
            api_key_env: None,
        }
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProfile {
    #[serde(default)]
    pub reasoning_effort: Option<ThinkingLevel>,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub auto_context_window: bool,
    #[serde(default = "default_context_tokens")]
    pub context_tokens: usize,
    pub output_tokens: u32,
    #[serde(default)]
    pub output_token_parameter: OutputTokenParameter,
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelSettings {
    pub default: String,
    pub providers: BTreeMap<String, ProviderSettings>,
    pub profiles: BTreeMap<String, ModelProfile>,
}
fn default_context_tokens() -> usize {
    crate::ContextSettings::default().context_tokens
}
impl ModelSettings {
    pub fn resolve_environment(&mut self, values: &HashMap<String, String>) {
        for provider in self.providers.values_mut() {
            if let Some(value) = provider
                .api_key_env
                .as_ref()
                .and_then(|key| values.get(key))
            {
                provider.api_key = SecretString::new(value.clone());
            }
        }
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (self.profiles.is_empty() && self.default.is_empty() && !self.providers.is_empty())
                || self.profiles.contains_key(&self.default),
            "models.default must name an existing profile"
        );
        for provider in self.providers.values() {
            validate_provider(provider)?;
        }
        for (name, profile) in &self.profiles {
            ensure!(
                !name.is_empty()
                    && name.len() <= 128
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
                "invalid model profile name"
            );
            let provider = self
                .providers
                .get(&profile.provider)
                .ok_or_else(|| anyhow::anyhow!("unknown provider for model profile {name}"))?;
            validate_provider(provider)?;
            ensure!(
                !profile.model.trim().is_empty()
                    && profile.output_tokens > 0
                    && profile.context_tokens > profile.output_tokens as usize,
                "invalid model profile {name}"
            );
        }
        Ok(())
    }
}
pub fn validate_provider(provider: &ProviderSettings) -> Result<()> {
    let url = url::Url::parse(&provider.base_url)
        .map_err(|_| anyhow::anyhow!("model base_url must be an absolute HTTP(S) URL"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "model base_url must be HTTP(S) without credentials, query or fragment"
    );
    ensure!(
        provider.adapter.is_local() || !provider.api_key.is_empty(),
        "LLM API key is missing"
    );
    ensure!(
        !provider.api_key.expose().chars().any(char::is_control),
        "LLM API key contains invalid control characters"
    );
    Ok(())
}
impl AppConfig {
    pub fn effective_models(&self) -> ModelSettings {
        if !self.models.profiles.is_empty()
            || !self.models.providers.is_empty()
            || !self.models.default.is_empty()
        {
            return self.models.clone();
        }
        ModelSettings {
            default: "default".into(),
            providers: [(
                "default".into(),
                ProviderSettings {
                    base_url: self.llm.base_url.clone(),
                    api_key: self.llm.api_key.clone(),
                    api_key_env: None,
                    adapter: self.llm.adapter,
                    protocol: self.llm.protocol,
                    discover_models: self.llm.discover_models,
                },
            )]
            .into(),
            profiles: [(
                "default".into(),
                ModelProfile {
                    reasoning_effort: self.llm.reasoning_effort,
                    capabilities: self.llm.capabilities.clone(),
                    provider: "default".into(),
                    model: self.llm.model.clone(),
                    auto_context_window: self.context.auto_model_window,
                    context_tokens: self.context.context_tokens,
                    output_tokens: self.context.output_tokens,
                    output_token_parameter: self.llm.output_token_parameter,
                },
            )]
            .into(),
        }
    }
}
