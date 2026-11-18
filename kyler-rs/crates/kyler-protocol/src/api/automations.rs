//! Durable automation definitions and run history, without provider credentials.
use super::{models::ThinkingLevel, permissions::PermissionMode, session::WorkspaceMode};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskConfig {
    pub name: String,
    pub prompt: String,
    pub cwd: String,
    #[serde(default)]
    pub workspace_mode: WorkspaceMode,
    pub cron: String,
    pub timezone: String,
    #[serde(default)]
    pub model_profile: Option<String>,
    pub thinking: ThinkingLevel,
    #[serde(default)]
    pub permission: PermissionMode,
    pub timeout_seconds: u32,
    pub wait_timeout_seconds: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: String,
    pub revision: u64,
    pub config: TaskConfig,
    pub enabled: bool,
    pub deleted: bool,
    pub next_run_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub latest_run: Option<ScheduledRun>,
    pub has_unfinished_run: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledRunStatus {
    Queued,
    Starting,
    Running,
    WaitingForApproval,
    WaitingForInput,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
    Skipped,
}
impl ScheduledRunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted | Self::Skipped
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledRun {
    pub id: String,
    pub task_id: String,
    pub task_revision: u64,
    pub snapshot: TaskConfig,
    pub trigger: String,
    pub scheduled_for: i64,
    pub status: ScheduledRunStatus,
    /// Assigned before admission; repeated recovery never creates a replacement session.
    pub session_id: String,
    pub core_run_id: Option<String>,
    pub execution_id: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub last_observed_at: i64,
    pub active_elapsed_ms: i64,
    pub waiting_since: Option<i64>,
    pub cancel_requested: bool,
    pub timed_out: bool,
    pub summary: Option<String>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveTaskRequest {
    pub config: TaskConfig,
    pub enabled: bool,
    pub expected_revision: Option<u64>,
    pub request_id: Option<String>,
    #[serde(default)]
    pub confirm_full_access: bool,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetTaskEnabled {
    pub enabled: bool,
    pub expected_revision: u64,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunTaskRequest {
    pub request_id: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulePreviewRequest {
    pub cron: String,
    pub timezone: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulePreview {
    pub occurrences: Vec<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledRunPage {
    pub runs: Vec<ScheduledRun>,
    pub next_cursor: Option<String>,
}
