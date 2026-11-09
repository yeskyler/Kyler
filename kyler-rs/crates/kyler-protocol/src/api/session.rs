use super::run::RunStatus;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    #[serde(default)]
    pub draft: bool,
    pub session_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub workspace_root: Option<String>,
    #[serde(default)]
    pub working_directory: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub event_count: usize,
    pub execution_count: usize,
    pub status: Option<RunStatus>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub execution_id: String,
    pub timestamp: i64,
    pub role: String,
    pub content: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagePage {
    pub messages: Vec<Message>,
    pub next_cursor: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPage {
    pub sessions: Vec<SessionSummary>,
    pub next_cursor: Option<usize>,
}

/// Select an execution layout within the server-authorized workspace boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    #[default]
    Isolated,
    Project,
}
impl WorkspaceMode {
    pub fn is_isolated(&self) -> bool {
        *self == Self::Isolated
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSessionRequest {
    #[serde(default, skip_serializing_if = "WorkspaceMode::is_isolated")]
    pub workspace_mode: WorkspaceMode,
    #[serde(default, skip_serializing_if = "is_false")]
    pub draft: bool,
    pub workspace_root: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}
