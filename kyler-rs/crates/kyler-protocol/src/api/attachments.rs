//! Session-owned attachments, shared by terminal and web clients.
use serde::{Deserialize, Serialize};

pub const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_ATTACHMENTS: usize = 8;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    Text,
    Pdf,
    Document,
    Spreadsheet,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub id: String,
    pub name: String,
    pub kind: AttachmentKind,
    pub bytes: usize,
    pub sha256: String,
    pub created_at: i64,
}
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadAttachmentRequest {
    pub attachment_id: String,
    #[serde(default)]
    pub unit: usize,
    #[serde(default)]
    pub offset: usize,
    pub limit: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentContent {
    pub info: AttachmentInfo,
    pub unit: usize,
    pub units: usize,
    pub unit_name: String,
    pub offset: usize,
    pub total_lines: usize,
    pub content: String,
    pub truncated: bool,
}
