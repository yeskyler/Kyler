//! Credential-free model catalog and session selection API.
use serde::{Deserialize, Serialize};

/// None means undeclared; existing providers remain compatible until configured.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ModelCapabilities {
    pub images: Option<bool>,
    pub tools: Option<bool>,
    pub structured_output: Option<bool>,
    pub tokenizer: Tokenizer,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Tokenizer {
    #[default]
    Conservative,
    Cl100kBase,
    O200kBase,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    pub profile: String,
    pub provider: String,
    pub model: String,
    pub revision: u64,
    pub context_window_tokens: usize,
    pub sliding_window_tokens: usize,
    pub output_tokens: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCatalog {
    pub revision: u64,
    pub default_profile: String,
    pub models: Vec<ModelInfo>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSelection {
    pub profile: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionModel {
    pub profile: Option<String>,
    pub selected: Option<ModelInfo>,
    pub active: Option<ModelInfo>,
    pub error: Option<String>,
}

/// Editable connection metadata. The key is deliberately never returned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConnectionSettings {
    pub url: String,
    pub model: String,
    pub key_configured: bool,
}

/// No Debug implementation: the request contains an optional credential.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConnectionUpdate {
    pub url: String,
    pub model: String,
    #[serde(default)]
    pub key: Option<String>,
}
