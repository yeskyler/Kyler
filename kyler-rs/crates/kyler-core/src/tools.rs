pub mod apply_patch;
mod arguments;
pub mod attachment;
pub mod cancel_task;
mod context;
pub mod delegate_task;
pub mod exec_command;
pub mod grep_files;
pub mod install_skill;
pub mod list_dir;
pub mod list_skills;
pub mod load_skill;
pub mod memory;
pub mod plan;
pub mod read_file;
pub mod register_artifacts;
pub mod save_skill;
pub(crate) mod search_tool_bm25;
pub mod shell;
pub mod skills_resource;
pub mod wait_tasks;

use self::{
    apply_patch::ApplyPatchTool, grep_files::GrepFilesTool, list_dir::ListDirTool,
    plan::UpdatePlanTool, read_file::ReadFileTool,
};
use crate::approval::ToolOperation;

use std::{collections::HashMap, sync::Arc};

use anyhow::{Result, ensure};
use serde_json::Value;

use kyler_config::AgentSettings;

pub use context::{ParallelToolContext, ToolContext};

pub use kyler_protocol::{
    activity::{ActivityKind, ToolSource},
    tool::ToolDefinition,
};

/// Only select whole entries. `render(0)` must truthfully describe omitted data.
pub(crate) fn fit_entries(
    total: usize,
    budget: &ToolOutputBudget,
    render: impl Fn(usize) -> String,
) -> Result<String> {
    budget.validate()?;
    let mut best = render(0);
    ensure!(
        best.len() <= budget.max_bytes,
        "tool metadata and omission notice cannot fit the output budget"
    );
    let (mut low, mut high) = (0, total);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let candidate = render(mid);
        if candidate.len() <= budget.max_bytes {
            low = mid;
            best = candidate;
        } else {
            high = mid - 1;
        }
    }
    Ok(best)
}

pub(crate) fn text_prefix(text: &str, max_bytes: usize) -> &str {
    let mut end = max_bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

pub type ToolBox = HashMap<String, Arc<dyn Tool>>;

#[derive(Debug, Clone, Copy)]
pub struct ToolOutputBudget {
    pub max_bytes: usize,
}
impl Default for ToolOutputBudget {
    fn default() -> Self {
        Self {
            max_bytes: AgentSettings::default().tool_output_max_bytes,
        }
    }
}
impl ToolOutputBudget {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.max_bytes > 0,
            "tool output byte budget must be positive"
        );
        Ok(())
    }
}

#[derive(Debug)]
pub struct CompressedToolOutput {
    pub content: String,
    pub compressed: bool,
}
impl CompressedToolOutput {
    pub fn unchanged(output: &str) -> Self {
        Self {
            content: output.to_string(),
            compressed: false,
        }
    }
    pub fn compressed(original: &str, content: String, budget: &ToolOutputBudget) -> Result<Self> {
        ensure!(
            !content.trim().is_empty()
                && content.len() <= budget.max_bytes
                && content.len() < original.len(),
            "compressed tool output must fit the budget and be smaller than the original"
        );
        Ok(Self {
            content,
            compressed: true,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Standard,
    SubagentManagement,
}

pub fn is_subagent_management(tool: &dyn Tool) -> bool {
    tool.kind() == ToolKind::SubagentManagement
        || matches!(
            tool.name(),
            "delegate_task" | "get_task" | "wait_tasks" | "cancel_task"
        )
}

/// Unknown tools fail closed when an environment is sandboxed. Host is reserved
/// for trusted state-only/control-plane tools; Worker executes through MXC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    /// Calls an already-authorized, session-bound MCP connection.
    Mcp,
    Unspecified,
    Host,
    Worker,
    /// Host-managed lifetime, with every command executed through the sandbox worker.
    ManagedProcess,
}

impl ToolExecutionMode {
    pub(crate) fn sandboxed(self) -> bool {
        matches!(self, Self::Worker | Self::ManagedProcess)
    }
}

/// Runtime scheduling policy, independent of sandbox permissions and approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolConcurrency {
    Exclusive,
    Parallel,
    /// A bounded wait uses a separate lane rather than command execution slots.
    Wait,
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::Exclusive
    }

    async fn execute_parallel(
        &self,
        _args_json: &str,
        _context: &ParallelToolContext<'_>,
    ) -> anyhow::Result<String> {
        anyhow::bail!(
            "tool {} has no parallel execution implementation",
            self.name()
        )
    }

    fn validate_arguments(&self, arguments: &Value) -> anyhow::Result<()> {
        arguments::validate(&self.parameters(), arguments)
    }
    fn approval_requirement(&self, _arguments: &Value) -> anyhow::Result<ToolOperation> {
        Ok(ToolOperation::External)
    }
    fn approval_identity(&self) -> String {
        format!(
            "{}:{}:{}",
            self.name(),
            self.description(),
            self.parameters()
        )
    }

    fn source_for_arguments(&self, _arguments: &Value) -> ToolSource {
        self.source()
    }
    fn source(&self) -> ToolSource {
        ToolSource::Builtin
    }
    fn activity_kind(&self) -> ActivityKind {
        ActivityKind::Think
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        if self.kind() == ToolKind::SubagentManagement {
            ToolExecutionMode::Host
        } else {
            ToolExecutionMode::Unspecified
        }
    }
    fn kind(&self) -> ToolKind {
        ToolKind::Standard
    }
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;

    async fn execute(
        &self,
        args_json: &str,
        context: &mut ToolContext<'_>,
    ) -> anyhow::Result<String>;

    /// Reduce a successful result for model input. Original output is retained.
    /// The default deliberately leaves small/action-oriented tools unchanged.
    fn compress_output(
        &self,
        output: &str,
        _budget: &ToolOutputBudget,
    ) -> anyhow::Result<CompressedToolOutput> {
        Ok(CompressedToolOutput::unchanged(output))
    }

    fn definition(&self) -> anyhow::Result<ToolDefinition> {
        Ok(ToolDefinition {
            name: self.name().into(),
            description: self.description().into(),
            parameters: self.parameters(),
        })
    }
}

pub async fn build_toolbox() -> anyhow::Result<ToolBox> {
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(UpdatePlanTool),
        Arc::new(register_artifacts::RegisterArtifactsTool),
        Arc::new(ListDirTool),
        Arc::new(ReadFileTool),
        Arc::new(GrepFilesTool),
        Arc::new(ApplyPatchTool),
    ];

    Ok(tools
        .into_iter()
        .map(|tool| (tool.name().to_string(), tool))
        .collect())
}

pub fn subagent_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(delegate_task::DelegateTaskTool),
        Arc::new(wait_tasks::WaitTasksTool),
        Arc::new(cancel_task::CancelTaskTool),
    ]
}

#[cfg(test)]
mod activity_tests {
    use super::{Tool, build_toolbox, shell, subagent_tools};

    use kyler_protocol::activity::ActivityKind;

    #[tokio::test]
    async fn builtins_declare_presentation_activity() {
        let tools = build_toolbox().await.unwrap();
        for (name, expected) in [
            ("read_file", ActivityKind::Explore),
            ("list_dir", ActivityKind::Explore),
            ("grep_files", ActivityKind::Explore),
            ("apply_patch", ActivityKind::Edit),
            ("update_plan", ActivityKind::Think),
        ] {
            assert_eq!(tools[name].activity_kind(), expected, "{name}");
        }
        assert_eq!(shell::RunCommandTool.activity_kind(), ActivityKind::Execute);
        for tool in subagent_tools() {
            assert_eq!(tool.activity_kind(), ActivityKind::Think);
        }
    }
}
