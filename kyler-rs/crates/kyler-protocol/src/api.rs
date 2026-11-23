pub mod artifacts;
pub mod attachments;
pub mod automations;
pub mod compaction;
pub mod event;
pub mod gateway;
pub mod history;
pub mod images;
pub mod inspection;
pub mod mcp;
pub mod memory;
pub mod models;
pub mod permissions;
pub mod preferences;
pub mod processes;
pub mod questions;
pub mod run;
pub mod session;
pub mod skills;
pub mod tasks;
pub mod tools;
pub mod workspaces;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ModelFailureDetails>,
    pub code: String,
    pub message: String,
}

/// Credential-free request diagnostics shared by every channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelFailureDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<models::WireProtocol>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameter: Option<String>,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// Effective runtime values; never includes credentials or model endpoint URLs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    #[serde(default)]
    pub instance_id: Option<String>,
    #[serde(default)]
    pub subagents_enabled: bool,
    pub model: String,
    pub context_window_tokens: usize,
    /// None for older servers; clients must not infer a retention ratio.
    #[serde(default)]
    pub sliding_window_tokens: Option<usize>,
    #[serde(default)]
    pub default_workspace: Option<String>,
    pub cwd: Option<String>,
}
/// Persisted session-wide provider usage, including accounted delegated work.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}
