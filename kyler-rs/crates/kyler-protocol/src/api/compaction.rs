//! Public lifecycle of a context compaction; never contains summary text.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionInfo {
    pub id: String,
    pub status: CompactionStatus,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

impl CompactionStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "Compacting context…",
            Self::Completed => "Context compacted",
            Self::Failed => "Context compaction failed",
            Self::Cancelled => "Context compaction cancelled",
            Self::Interrupted => "Context compaction interrupted",
        }
    }
}
