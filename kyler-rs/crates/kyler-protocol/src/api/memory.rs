//! Scoped memory records and explicit review contracts.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Candidate,
    #[default]
    Active,
    Rejected,
    Invalid,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    #[default]
    Fact,
    Preference,
    Lesson,
    Procedure,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryMetadata {
    pub kind: MemoryKind,
    pub conditions: String,
    pub evidence: Vec<String>,
    pub expires_at: Option<i64>,
    pub sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    #[serde(default)]
    pub status: MemoryStatus,
    #[serde(default)]
    pub metadata: MemoryMetadata,
    #[serde(default)]
    pub source_execution: Option<String>,
    #[serde(default)]
    pub source_tool_call: Option<String>,
    #[serde(default)]
    pub reviewed_at: Option<i64>,
    pub key: String,
    pub content: String,
    pub tags: Vec<String>,
    pub revision: u64,
    pub updated_at: i64,
    pub source_session: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryReviewPage {
    pub scope: String,
    pub entries: Vec<MemoryEntry>,
    pub total: usize,
    pub next_cursor: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryReviewDetails {
    pub scope: String,
    pub entry: MemoryEntry,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryReviewRequest {
    pub key: String,
    pub expected_scope: String,
    pub expected_revision: u64,
    pub status: MemoryStatus,
}
