use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub id: String,
    pub path: String,
    pub title: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveWorkspaceRequest {
    pub path: String,
    pub title: String,
}

/// A workspace view retains each file's owning session for authorized downloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInput {
    pub session_id: String,
    pub attachment: super::attachments::AttachmentInfo,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceFiles {
    pub inputs: Vec<WorkspaceInput>,
    pub outputs: Vec<super::artifacts::ArtifactInfo>,
}
