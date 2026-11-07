use crate::{
    approval::ToolOperation,
    subagent::DelegateTaskArgs,
    tools::{
        CompressedToolOutput, Tool, ToolContext, ToolKind, ToolOutputBudget,
        output::compress_task_json,
    },
};

use anyhow::Context;
use schemars::schema_for;
use serde_json::Value;

pub struct DelegateTaskTool;
#[async_trait::async_trait]
impl Tool for DelegateTaskTool {
    fn approval_requirement(&self, _: &Value) -> anyhow::Result<ToolOperation> {
        Ok(ToolOperation::State)
    }

    fn kind(&self) -> ToolKind {
        ToolKind::SubagentManagement
    }
    fn name(&self) -> &str {
        "delegate_task"
    }
    fn description(&self) -> &str {
        "Set agent=review with review.target for a read-only code review of uncommitted, base_branch, or commit changes; review reports are structured and never apply fixes. Otherwise agent defaults to general. Start an independent child agent in the background and immediately return its task ID. Give a self-contained task and necessary context. It inherits ordinary tools but cannot manage subagents. Use get_task/wait_tasks to collect results before answering; unfinished tasks are cancelled when this parent execution ends."
    }
    fn validate_arguments(&self, arguments: &Value) -> anyhow::Result<()> {
        serde_json::from_value::<DelegateTaskArgs>(arguments.clone())?.validate()
    }
    fn parameters(&self) -> Value {
        serde_json::to_value(schema_for!(DelegateTaskArgs)).expect("tool schema is serializable")
    }
    async fn execute(
        &self,
        args_json: &str,
        context: &mut ToolContext<'_>,
    ) -> anyhow::Result<String> {
        let args: DelegateTaskArgs = serde_json::from_str(args_json)?;
        let scope = context
            .subagents
            .context("subagent manager is not configured")?;
        Ok(serde_json::to_string(
            &scope.delegate(args, context.tool_call_id)?,
        )?)
    }
    fn compress_output(
        &self,
        output: &str,
        budget: &ToolOutputBudget,
    ) -> anyhow::Result<CompressedToolOutput> {
        compress_task_json(output, budget)
    }
}
