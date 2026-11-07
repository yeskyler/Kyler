//! State for one agent execution within a session.
use crate::context::{ExecutionPlan, TokenUsage};

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use kyler_protocol::api::{
    history::ToolPresentation,
    models::ModelInfo,
    permissions::{ApprovalRecord, PermissionSnapshot},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
    pub execution_id: String,
    #[serde(default)]
    pub approvals: Vec<ApprovalRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<PermissionSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_snapshot: Option<ModelInfo>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tool_presentations: HashMap<String, ToolPresentation>,
    pub current_step: u32,
    pub plan: Option<ExecutionPlan>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deliverables: Vec<crate::artifacts::Deliverable>,
    pub final_result: Option<String>,
    pub usage: TokenUsage,
    #[serde(default)]
    pub subagent_usage: TokenUsage,
    pub status: ExecutionStatus,
    pub error: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

impl ExecutionContext {
    pub fn new() -> Self {
        Self {
            resumed_from: None,
            execution_id: Uuid::new_v4().to_string(),
            approvals: Vec::new(),
            permissions: None,
            model: None,
            model_snapshot: None,
            tool_presentations: HashMap::new(),
            current_step: 0,
            plan: None,
            deliverables: Vec::new(),
            final_result: None,
            usage: TokenUsage::default(),
            subagent_usage: TokenUsage::default(),
            status: ExecutionStatus::Running,
            error: None,
            started_at: chrono::Utc::now().timestamp_millis(),
            finished_at: None,
        }
    }
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self::new()
    }
}
