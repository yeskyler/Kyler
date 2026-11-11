//! Agent execution, session history, context compaction and tools.
pub mod approval;
pub mod artifacts;
pub mod attachments;
pub mod context;
mod diagnostics;
pub mod environment;
pub mod mcp;
pub mod memory;
pub mod models;
pub mod processes;
pub mod questions;
pub mod rollout;
pub mod runtime;
pub mod sandbox;
pub mod session;
pub mod skills;
pub mod subagent;
pub mod tools;

/// Compatibility path for image input types.
pub use attachments::image as images;
pub use context::{
    ContextCompactor, ContextMemory, ContextPolicy, ExecutionContext, ExecutionStatus, TokenUsage,
};
pub use environment::{ExecutionEnvironment, SessionEnvironment};
pub use rollout::{RolloutPayload, RolloutPosition, RolloutReader, RolloutRecord, RolloutWriter};
pub use runtime::{
    Agent, AgentCancelHandle, AgentResult, AgentStream, AgentStreamEvent, AgentStreamOutcome,
};
pub use sandbox::{FilesystemPolicy, MxcSandbox, NetworkAccess, ProcessPolicy, SandboxPolicy};
pub use session::{Session, SessionInfo, SessionStore};
pub use subagent::{SubagentConfig, SubagentManager, SubagentStatus, SubagentTask};
pub use tools::{
    CompressedToolOutput, Tool, ToolBox, ToolContext, ToolDefinition, ToolExecutionMode, ToolKind,
    ToolOutputBudget,
};

pub mod thread;
