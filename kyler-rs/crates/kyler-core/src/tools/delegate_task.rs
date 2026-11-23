use crate::{
    approval::ToolOperation,
    subagent::{DelegateTaskArgs, compress_task_json},
    tools::{CompressedToolOutput, Tool, ToolContext, ToolKind, ToolOutputBudget},
};

use anyhow::Context;
use schemars::schema_for;
use serde_json::Value;

pub struct DelegateTaskTool;
#[async_trait::async_trait]
impl Tool for DelegateTaskTool {
    fn approval_requirement(&self, args: &Value) -> anyhow::Result<ToolOperation> {
        Ok(if args["agent"] == "memory" {
            ToolOperation::Write
        } else {
            ToolOperation::State
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::SubagentManagement
    }
    fn name(&self) -> &str {
        "delegate_task"
    }
    fn description(&self) -> &str {
        "Set agent=review with review.target for a read-only code review of uncommitted, base_branch, or commit changes; review reports are structured and never apply fixes. Use agent=memory to review prior committed messages for scoped memory candidates and reusable procedures when the user requests it or the Memory guidance calls for a focused history review. Otherwise agent defaults to general. Start an independent child agent in the background and immediately return its task ID. Give a self-contained task and necessary context. General tasks inherit ordinary tools. No child can manage subagents. Use wait_tasks to collect results before answering; unfinished tasks are cancelled when this parent execution ends."
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
