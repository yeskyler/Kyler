use super::{ApiErrorBody, run::Usage};
use crate::activity::{ActivityKind, ToolSource};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerEvent {
    pub run_id: String,
    pub execution_id: Option<String>,
    pub seq: u64,
    pub timestamp_ms: i64,
    #[serde(flatten)]
    pub payload: EventPayload,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    QuestionUpdated {
        question: super::questions::QuestionRecord,
    },
    ApprovalUpdated {
        approval: super::permissions::ApprovalRecord,
    },
    PermissionsBound {
        policy: super::permissions::PermissionSnapshot,
    },
    RunStarted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<super::models::ModelInfo>,
        session_id: String,
    },
    StepStarted {
        step: u32,
    },
    ReasoningDelta {
        step: u32,
        text: String,
    },
    TextDelta {
        step: u32,
        text: String,
    },
    ToolQueued {
        call_id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default)]
        activity: ActivityKind,
        #[serde(default)]
        source: ToolSource,
    },
    ToolStarted {
        call_id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default)]
        activity: ActivityKind,
        #[serde(default)]
        source: ToolSource,
    },
    ToolFinished {
        call_id: String,
        name: String,
        status: String,
        output_bytes: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    RunCompleted {
        output: String,
        output_truncated: bool,
        usage: Usage,
    },
    RunFailed {
        error: ApiErrorBody,
    },
    RunCancelled,
}
impl EventPayload {
    pub fn name(&self) -> &'static str {
        match self {
            Self::QuestionUpdated { .. } => "question_updated",
            Self::ApprovalUpdated { .. } => "approval_updated",
            Self::PermissionsBound { .. } => "permissions_bound",
            Self::RunStarted { .. } => "run_started",
            Self::StepStarted { .. } => "step_started",
            Self::ReasoningDelta { .. } => "reasoning_delta",
            Self::TextDelta { .. } => "text_delta",
            Self::ToolQueued { .. } => "tool_queued",
            Self::ToolStarted { .. } => "tool_started",
            Self::ToolFinished { .. } => "tool_finished",
            Self::RunCompleted { .. } => "run_completed",
            Self::RunFailed { .. } => "run_failed",
            Self::RunCancelled => "run_cancelled",
        }
    }
}
