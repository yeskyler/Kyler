pub mod artifacts;
pub mod attachments;
pub mod event;
pub mod history;
pub mod images;
pub mod inspection;
pub mod mcp;
pub mod models;
pub mod permissions;
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
    pub code: String,
    pub message: String,
}

/// Effective runtime values; never includes credentials or model endpoint URLs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    #[serde(default)]
    pub instance_id: Option<String>,
    pub model: String,
    pub context_window_tokens: usize,
    /// None for older servers; clients must not infer a retention ratio.
    #[serde(default)]
    pub sliding_window_tokens: Option<usize>,
    #[serde(default)]
    pub default_workspace: Option<String>,
    pub workspace_root: Option<String>,
    pub working_directory: Option<String>,
}
/// Persisted session-wide provider usage, including accounted delegated work.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}
