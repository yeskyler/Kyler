//! Session-scoped background task presentation shared by terminal and web clients.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Command,
    Agent,
}
impl TaskKind {
    pub fn path(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Agent => "agent",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: String,
    pub kind: TaskKind,
    pub session_id: String,
    pub name: String,
    pub status: String,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub working_directory: Option<String>,
    pub exit_code: Option<i32>,
    pub total_tokens: Option<u64>,
    pub output: Option<String>,
    pub output_truncated: bool,
    pub error: Option<String>,
    pub children: Vec<TaskInfo>,
}
impl TaskInfo {
    pub fn active(&self) -> bool {
        matches!(self.status.as_str(), "running" | "queued")
    }
    pub fn failed(&self) -> bool {
        matches!(self.status.as_str(), "failed" | "timed_out" | "interrupted")
    }
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskAction {
    Stop,
    Input {
        data: String,
        #[serde(default)]
        close: bool,
    },
}
