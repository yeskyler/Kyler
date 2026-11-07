//! Configured OpenAI-compatible providers and named model profiles.
use crate::{AppConfig, SecretString};

use std::collections::{BTreeMap, HashMap};

use anyhow::{Result, ensure};
use serde::Deserialize;

use kyler_protocol::api::models::ModelCapabilities;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputTokenParameter {
    MaxTokens,
    #[default]
    MaxCompletionTokens,
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderSettings {
    pub base_url: String,
    pub api_key: SecretString,
    pub api_key_env: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProfile {
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    pub provider: String,
    pub model: String,
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
            self.profiles.contains_key(&self.default),
            "models.default must name an existing profile"
        );
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
    ensure!(!provider.api_key.is_empty(), "LLM API key is missing");
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
                },
            )]
            .into(),
            profiles: [(
                "default".into(),
                ModelProfile {
                    capabilities: self.llm.capabilities.clone(),
                    provider: "default".into(),
                    model: self.llm.model.clone(),
                    context_tokens: self.context.context_tokens,
                    output_tokens: self.context.output_tokens,
                    output_token_parameter: OutputTokenParameter::MaxTokens,
                },
            )]
            .into(),
        }
    }
}
