//! Approval controls prompts and decisions; it never changes a sandbox boundary.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalPolicy {
    Untrusted,
    #[default]
    OnRequest,
    OnFailure,
    Never,
    Reject,
}
impl ApprovalPolicy {
    pub const SELECTABLE: [Self; 4] = [Self::Untrusted, Self::OnRequest, Self::Never, Self::Reject];
    pub fn label(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::OnRequest => "on-request",
            Self::OnFailure => "on-failure (deprecated)",
            Self::Never => "never",
            Self::Reject => "reject",
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalReviewer {
    #[default]
    User,
    AutoReview,
}
impl ApprovalReviewer {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::AutoReview => "auto_review",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRule {
    Allow,
    Deny,
    Ask,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RejectedRequests {
    pub sandbox_escalation: bool,
    pub tool_rules: bool,
    pub mcp_elicitation: bool,
}
impl Default for RejectedRequests {
    fn default() -> Self {
        Self {
            sandbox_escalation: true,
            tool_rules: true,
            mcp_elicitation: true,
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApprovalSettings {
    pub policy: ApprovalPolicy,
    pub reviewer: ApprovalReviewer,
    pub reject: RejectedRequests,
    pub rules: BTreeMap<String, ToolRule>,
}
impl ApprovalSettings {
    pub fn validation_error(&self) -> Option<&'static str> {
        if self.policy == ApprovalPolicy::OnFailure {
            return Some(
                "on-failure is deprecated; use on-request. Kyler never retries outside its sandbox automatically",
            );
        }
        if self.rules.len() > 128
            || self.rules.keys().any(|name| {
                name.is_empty() || name.len() > 256 || name.chars().any(char::is_control)
            })
        {
            return Some("invalid approval tool rules");
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalRequestKind {
    SandboxEscalation,
    ToolRule,
    McpElicitation,
}
impl ApprovalSettings {
    pub fn rejects(&self, kind: ApprovalRequestKind) -> bool {
        if self.policy == ApprovalPolicy::Never {
            return true;
        }
        self.policy == ApprovalPolicy::Reject
            && match kind {
                ApprovalRequestKind::SandboxEscalation => self.reject.sandbox_escalation,
                ApprovalRequestKind::ToolRule => self.reject.tool_rules,
                ApprovalRequestKind::McpElicitation => self.reject.mcp_elicitation,
            }
    }
}

#[cfg(test)]
mod tests {
    use super::{ApprovalPolicy, ApprovalRequestKind, ApprovalSettings};

    #[test]
    fn rejected_request_categories_are_independent() {
        let mut settings = ApprovalSettings {
            policy: ApprovalPolicy::Reject,
            ..Default::default()
        };
        for kind in [
            ApprovalRequestKind::SandboxEscalation,
            ApprovalRequestKind::ToolRule,
            ApprovalRequestKind::McpElicitation,
        ] {
            assert!(settings.rejects(kind));
        }
        settings.reject.sandbox_escalation = false;
        settings.reject.mcp_elicitation = false;
        assert!(!settings.rejects(ApprovalRequestKind::SandboxEscalation));
        assert!(!settings.rejects(ApprovalRequestKind::McpElicitation));
        assert!(settings.rejects(ApprovalRequestKind::ToolRule));
        settings.policy = ApprovalPolicy::Never;
        assert!(settings.rejects(ApprovalRequestKind::SandboxEscalation));
    }
}
