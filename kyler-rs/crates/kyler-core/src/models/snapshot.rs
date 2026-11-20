//! Immutable model and context-budget settings bound to an execution.
use crate::context::ContextPolicy;

use kyler_config::{OutputTokenParameter, ProviderSettings, SecretString};
use kyler_protocol::api::models::{ModelConnectionSettings, ModelInfo};

#[derive(Clone)]
pub struct ModelSnapshot {
    pub adapter: kyler_provider::Adapter,
    pub(crate) protocol_kind: kyler_provider::WireProtocol,
    pub(crate) adapter_kind: kyler_provider::AdapterKind,
    pub(crate) discover_models: bool,
    pub info: ModelInfo,
    pub policy: ContextPolicy,
    pub output_token_parameter: OutputTokenParameter,
    pub reasoning_effort: Option<kyler_protocol::api::models::ThinkingLevel>,
    pub(crate) base_url: String,
    pub(crate) api_key: SecretString,
}

impl ModelSnapshot {
    /// Connection and protocol are part of preference identity; credentials are not.
    pub fn preference_key(&self) -> String {
        use sha2::{Digest, Sha256};
        format!(
            "{:x}",
            Sha256::digest(format!(
                "{}\0{}\0{}\0{:?}\0{:?}",
                self.info.provider,
                self.base_url.trim_end_matches('/'),
                self.info.model,
                self.info.protocol,
                self.adapter.0
            ))
        )
    }

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
            protocol: self.protocol_kind,
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            api_key_env: None,
            adapter: self.adapter_kind,
            discover_models: self.discover_models,
        }
    }
}
