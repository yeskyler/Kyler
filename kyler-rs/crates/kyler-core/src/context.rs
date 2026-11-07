//! Execution state, history, model messages and context compaction.
pub mod compactor;
pub mod event;
pub mod execution;
pub mod history;
pub(crate) mod messages;
pub mod plan;
pub mod policy;
pub(crate) mod summarizer;
pub mod usage;

pub use compactor::ContextCompactor;
pub use event::{ContentItem, Event, EventAuthor, Role, ToolResultStatus};
pub use execution::{ExecutionContext, ExecutionStatus};
pub use history::ContextMemory;
pub use plan::{ExecutionPlan, PlanStep, PlanStepStatus};
pub use policy::ContextPolicy;
pub use usage::TokenUsage;

/// Compatibility path for conversation history.
pub use history as memory;

pub struct SummaryResult {
    pub text: String,
    pub usage: Option<(u32, u32, u32)>,
}
#[async_trait::async_trait]
pub trait Summarizer: Send {
    async fn summarize(&mut self, input: &str, max_tokens: u32) -> anyhow::Result<SummaryResult>;
}
