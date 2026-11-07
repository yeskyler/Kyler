//! Immutable session deliverables registered explicitly or linked in a final assistant message.
use serde::{Deserialize, Serialize};

pub const MAX_ARTIFACT_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactInfo {
    pub id: String,
    pub session_id: String,
    pub execution_id: String,
    pub reference: String,
    /// Local dependency of a deliverable, hidden from the top-level output list.
    #[serde(default)]
    pub resource: bool,
    /// Explicitly selected main deliverable for automatic preview.
    #[serde(default)]
    pub primary: bool,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
    pub available: bool,
    pub error: Option<String>,
}
