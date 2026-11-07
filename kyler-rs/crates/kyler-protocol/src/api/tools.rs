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
