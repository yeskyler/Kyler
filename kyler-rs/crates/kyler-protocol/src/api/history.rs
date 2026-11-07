//! Durable, provider-independent conversation history for every client.
use super::attachments::AttachmentInfo;
use crate::activity::{ActivityKind, ToolSource};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPresentation {
    pub step: u32,
    pub activity: ActivityKind,
    pub source: ToolSource,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HistoryContent {
    Reasoning {
        text: String,
    },
    Message {
        role: String,
        text: String,
    },
    Attachment {
        attachment: AttachmentInfo,
    },
    /// Legacy inline images have no downloadable attachment identifier.
    Image {
        name: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        arguments: Value,
        summary: Option<String>,
        presentation: Option<ToolPresentation>,
    },
    ToolResult {
        call_id: String,
        name: String,
        status: String,
        output: String,
        output_bytes: usize,
        truncated: bool,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEvent {
    pub id: String,
    pub execution_id: String,
    pub timestamp_ms: i64,
    pub content: Vec<HistoryContent>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryExecution {
    pub execution_id: String,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryPage {
    pub events: Vec<HistoryEvent>,
    pub executions: Vec<HistoryExecution>,
    pub next_cursor: Option<usize>,
}
