//! Credential-free model catalog and session selection API.
use serde::{Deserialize, Serialize};

/// A provider's wire API, independently selectable from its branding.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WireProtocol {
    #[default]
    Auto,
    ChatCompletions,
    Responses,
}

/// None means undeclared; existing providers remain compatible until configured.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ModelCapabilities {
    pub images: Option<bool>,
    pub tools: Option<bool>,
    pub structured_output: Option<bool>,
    pub tokenizer: Tokenizer,
    pub reasoning: Option<ReasoningCapabilities>,
    pub reasoning_content: Option<bool>,
    pub max_context_tokens: Option<usize>,
    pub max_output_tokens: Option<u32>,
}
/// Explicitly supported levels; None on ModelCapabilities means unknown.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReasoningCapabilities {
    pub levels: Vec<ThinkingLevel>,
    pub default: ThinkingLevel,
    pub source: String,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Tokenizer {
    Cl100kBase,
    #[default]
    #[serde(alias = "conservative")]
    O200kBase,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    #[serde(default)]
    pub protocol: WireProtocol,
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
    #[serde(default)]
    pub providers: Vec<ProviderDiscoveryState>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDiscoveryState {
    pub provider: String,
    pub status: String,
    pub error: Option<String>,
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

/// Per-session reasoning intent, independent of provider-specific request parameters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    #[serde(alias = "none")]
    Off,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}
impl ThinkingLevel {
    /// Provider spelling differs from the UI's historical "off" preference.
    pub fn reasoning_effort(self) -> &'static str {
        match self {
            Self::Off => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn from_enabled(enabled: bool) -> Self {
        if enabled { Self::High } else { Self::Off }
    }
    pub fn enabled(self) -> bool {
        self != Self::Off
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingState {
    #[serde(default)]
    pub follows_default: bool,
    pub enabled: bool,
    pub supported: bool,
    pub level: ThinkingLevel,
    pub levels: Vec<ThinkingLevel>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThinkingSelection {
    #[serde(default)]
    pub use_default: bool,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub level: Option<ThinkingLevel>,
}
impl ThinkingSelection {
    pub fn selected_level(&self) -> Result<Option<ThinkingLevel>, &'static str> {
        match (self.use_default, self.level, self.enabled) {
            (true, None, None) => Ok(None),
            (false, Some(level), None) => Ok(Some(level)),
            (false, None, Some(enabled)) => Ok(Some(ThinkingLevel::from_enabled(enabled))),
            _ => Err("Provide exactly one thinking level, use_default, or legacy enabled flag"),
        }
    }
}
