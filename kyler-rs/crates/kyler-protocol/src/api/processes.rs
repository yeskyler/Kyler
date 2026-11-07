use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessOutput {
    pub process_id: String,
    pub session_id: String,
    pub cursor: u64,
    pub stream: String,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessMetadata {
    pub command: String,
    pub working_directory: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ProcessMetadata>,
    pub id: String,
    pub status: String,
    pub cursor: u64,
    pub oldest_cursor: u64,
    pub output: Vec<ProcessOutput>,
    pub result: Option<String>,
    pub error: Option<String>,
}
