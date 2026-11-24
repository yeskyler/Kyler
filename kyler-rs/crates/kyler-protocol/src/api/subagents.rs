//! Persistent delegation settings and the currently active runtime snapshot.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentSettingsUpdate {
    pub enabled: bool,
    pub max_concurrent_tasks: usize,
    pub max_tasks_per_execution: usize,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct SubagentSettingsResponse {
    pub configured: SubagentSettingsUpdate,
    pub active: SubagentSettingsUpdate,
    pub restart_required: bool,
    pub max_concurrent_limit: usize,
    pub max_tasks_limit: usize,
}
