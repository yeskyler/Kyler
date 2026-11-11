//! Immutable model and context-budget settings bound to an execution.
use crate::context::ContextPolicy;

use kyler_config::{OutputTokenParameter, ProviderSettings, SecretString};
use kyler_protocol::api::models::{ModelConnectionSettings, ModelInfo};

#[derive(Clone)]
pub struct ModelSnapshot {
    pub info: ModelInfo,
    pub policy: ContextPolicy,
    pub output_token_parameter: OutputTokenParameter,
    pub enable_thinking: Option<bool>,
    pub(crate) base_url: String,
    pub(crate) api_key: SecretString,
}

impl ModelSnapshot {
    pub fn connection_settings(&self) -> ModelConnectionSettings {
        ModelConnectionSettings {
            url: self.base_url.clone(),
            model: self.info.model.clone(),
            key_configured: !self.api_key.is_empty(),
        }
    }
    /// Trusted host configuration access. SecretString redacts the credential in Debug.
    pub fn provider_settings(&self) -> ProviderSettings {
        ProviderSettings {
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            api_key_env: None,
        }
    }
}
