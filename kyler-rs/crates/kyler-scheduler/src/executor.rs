//! Adapter contract: the host owns sessions, permissions, and the agent operation queue.
use anyhow::Result;
use async_trait::async_trait;

use kyler_protocol::api::automations::{ScheduledRun, ScheduledRunStatus, TaskConfig};

#[derive(Debug, Clone)]
pub struct ExecutionRef {
    pub run_id: String,
    pub execution_id: Option<String>,
}
pub enum StartOutcome {
    Started(ExecutionRef),
    Deferred,
    Failed(String),
    Unknown(String),
}
pub struct ExecutionState {
    pub status: ScheduledRunStatus,
    pub reference: Option<ExecutionRef>,
    pub summary: Option<String>,
    pub error: Option<String>,
}
#[async_trait]
pub trait ScheduledExecutor: Send + Sync {
    /// Validate and canonicalize the configuration without starting an agent.
    async fn validate(&self, config: &mut TaskConfig) -> Result<()>;
    async fn start(&self, run: &ScheduledRun) -> StartOutcome;
    /// None means the admission outcome cannot be established; never blindly replay it.
    async fn inspect(&self, run: &ScheduledRun) -> Result<Option<ExecutionState>>;
    async fn cancel(&self, run: &ScheduledRun) -> Result<()>;
}
