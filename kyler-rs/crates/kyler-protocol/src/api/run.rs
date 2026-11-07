use super::ApiErrorBody;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRunRequest {
    pub input: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_ids: Vec<String>,
    #[serde(default)]
    pub resume: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<super::images::ImageInput>,
    pub client_request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Starting,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}
impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<super::permissions::PermissionSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<super::models::ModelInfo>,
    pub run_id: String,
    pub session_id: String,
    pub execution_id: Option<String>,
    pub status: RunStatus,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub last_event_seq: u64,
    pub output: Option<String>,
    pub usage: Option<Usage>,
    pub error: Option<ApiErrorBody>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunAccepted {
    pub run_id: String,
    pub session_id: String,
    pub status: RunStatus,
    pub events_url: String,
}
