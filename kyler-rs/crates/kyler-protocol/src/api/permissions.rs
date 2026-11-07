//! User-selected permission presets and exact tool-call approval records.
use crate::{
    approval::{ApprovalPolicy, ApprovalReviewer, ApprovalSettings},
    sandbox::SandboxMode,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// User-facing access presets. AutoReview is accepted only for legacy sessions.
pub enum PermissionMode {
    ReadOnly,
    #[default]
    WorkspaceAccess,
    AutoReview,
    FullAccess,
}
impl PermissionMode {
    pub const ALL: [Self; 3] = [Self::ReadOnly, Self::WorkspaceAccess, Self::FullAccess];
    pub fn preset(self) -> Self {
        if self == Self::AutoReview {
            Self::WorkspaceAccess
        } else {
            self
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "Read-only",
            Self::WorkspaceAccess => "Workspace access",
            Self::AutoReview => "Auto review",
            Self::FullAccess => "Full access",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionSelection {
    #[serde(default)]
    pub approval: Option<ApprovalSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<PermissionMode>,
    #[serde(default)]
    pub confirm_full_access: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalSettings>,
    pub mode: PermissionMode,
    pub revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionOption {
    pub mode: PermissionMode,
    pub available: bool,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionState {
    #[serde(default)]
    pub sandbox: Option<SandboxMode>,
    #[serde(default)]
    pub approval: ApprovalSettings,
    pub selected: PermissionSnapshot,
    pub active: Option<PermissionSnapshot>,
    pub options: Vec<PermissionOption>,
    pub workspace: Option<String>,
    pub ceiling: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Allowed,
    Denied,
    Expired,
    Cancelled,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub id: String,
    pub run_id: String,
    pub session_id: String,
    pub origin_session_id: String,
    pub execution_id: String,
    pub tool_call_id: String,
    pub tool: String,
    pub operation: String,
    pub preview: String,
    #[serde(default)]
    pub requires_user: bool,
    pub working_directory: Option<String>,
    pub policy: PermissionSnapshot,
    pub digest: String,
    pub status: ApprovalStatus,
    pub reason: String,
    pub created_at: i64,
    pub expires_at: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    AllowOnce,
    AllowSession,
    Deny,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalResponse {
    pub decision: ApprovalDecision,
    pub digest: String,
}

impl PermissionSnapshot {
    pub fn effective_approval(&self) -> ApprovalSettings {
        self.approval.clone().unwrap_or_else(|| ApprovalSettings {
            policy: if self.mode == PermissionMode::FullAccess {
                ApprovalPolicy::Never
            } else {
                ApprovalPolicy::OnRequest
            },
            reviewer: if self.mode == PermissionMode::AutoReview {
                ApprovalReviewer::AutoReview
            } else {
                ApprovalReviewer::User
            },
            ..Default::default()
        })
    }
}
