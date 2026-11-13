use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDetails {
    pub call_id: String,
    pub execution_id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub status: String,
    pub output: String,
    pub output_bytes: usize,
    pub truncated: bool,
}

/// Confirmed file changes from one apply_patch call. Paths are workspace-relative.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PatchFileChange {
    pub path: String,
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PatchResult {
    pub kind: String,
    pub status: String,
    pub files: Vec<PatchFileChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
impl PatchResult {
    pub fn from_output(output: &str) -> Option<Self> {
        let result: Self = serde_json::from_str(output).ok()?;
        (result.kind == "patch_result"
            && matches!(result.status.as_str(), "success" | "partial" | "error"))
        .then_some(result)
    }
}
