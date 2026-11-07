//! Read-only, session-scoped execution and context inspection.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBudget {
    pub model: String,
    pub context_tokens: usize,
    pub sliding_window_tokens: usize,
    pub input_limit: usize,
    pub output_tokens: u32,
    pub safety_tokens: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenComponent {
    pub name: String,
    pub estimated_tokens: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRequestSnapshot {
    pub execution_id: String,
    pub step: u32,
    pub timestamp_ms: i64,
    pub budget: ContextBudget,
    pub estimated_input_tokens: usize,
    pub provider_input_tokens: Option<usize>,
    /// Proportional estimates; JSON framing and provider tokenization are not additive.
    pub components: Vec<TokenComponent>,
    pub retrieved_memory_keys: Vec<String>,
    pub visible_events: usize,
    pub window_start: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionSnapshot {
    pub timestamp_ms: i64,
    pub before_tokens: usize,
    pub after_tokens: usize,
    pub window_start: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextInspection {
    pub session_id: String,
    pub latest_request: Option<ContextRequestSnapshot>,
    pub next_request_budget: Option<ContextBudget>,
    pub budget_error: Option<String>,
    pub compression_count: usize,
    pub last_compaction: Option<CompactionSnapshot>,
    pub window_start: usize,
    pub total_events: usize,
    pub cumulative_input_tokens: u64,
    pub cumulative_output_tokens: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolloutItem {
    pub seq: u64,
    pub timestamp_ms: i64,
    pub execution_id: Option<String>,
    pub kind: String,
    pub title: String,
    pub status: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolloutPage {
    pub session_id: String,
    pub items: Vec<RolloutItem>,
    pub cursor: u64,
    pub next_cursor: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolloutDetail {
    pub item: RolloutItem,
    pub data: Value,
}
