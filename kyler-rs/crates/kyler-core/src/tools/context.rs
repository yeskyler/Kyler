//! Narrow execution and session state made available to tools.
use crate::{
    context::ExecutionContext, environment::ExecutionEnvironment, processes::ProcessManager,
    subagent::SubagentScope,
};

use std::collections::HashMap;

use serde_json::Value;

/// Narrow mutable access for tools: execution state and cross-turn application state.
pub struct ToolContext<'a> {
    pub processes: Option<&'a ProcessManager>,
    pub execution: &'a mut ExecutionContext,
    pub state: &'a mut HashMap<String, Value>,
    pub session_id: &'a str,
    pub tool_call_id: &'a str,
    pub subagents: Option<&'a SubagentScope>,
    pub environment: Option<&'a ExecutionEnvironment>,
}

/// Shared execution services for tools that cannot mutate session state.
#[derive(Clone, Copy)]
pub struct ParallelToolContext<'a> {
    pub processes: Option<&'a ProcessManager>,
    pub session_id: &'a str,
    pub tool_call_id: &'a str,
    pub subagents: Option<&'a SubagentScope>,
    pub environment: Option<&'a ExecutionEnvironment>,
}
impl<'a> ToolContext<'a> {
    pub fn parallel(&self) -> ParallelToolContext<'_> {
        ParallelToolContext {
            processes: self.processes,
            session_id: self.session_id,
            tool_call_id: self.tool_call_id,
            subagents: self.subagents,
            environment: self.environment,
        }
    }
}
