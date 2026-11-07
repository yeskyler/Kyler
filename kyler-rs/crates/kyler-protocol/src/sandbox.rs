//! Host-selected sandbox boundaries, independent of per-tool approval decisions.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    ReadOnly,
    #[default]
    WorkspaceWrite,
    DangerFullAccess,
    ExternalSandbox,
}
impl SandboxMode {
    pub fn isolated(self) -> bool {
        matches!(self, Self::ReadOnly | Self::WorkspaceWrite)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
            Self::ExternalSandbox => "external-sandbox",
        }
    }
}
