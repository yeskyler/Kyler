use crate::{
    approval::ToolOperation,
    tools::{
        CompressedToolOutput, Tool, ToolContext, ToolKind, ToolOutputBudget,
        output::compress_task_json,
    },
};

use anyhow::Context;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CancelTaskArgs {
    pub task_id: String,
}

pub struct CancelTaskTool;
#[async_trait::async_trait]
impl Tool for CancelTaskTool {
    fn approval_requirement(&self, _: &Value) -> anyhow::Result<ToolOperation> {
        Ok(ToolOperation::State)
    }

    fn kind(&self) -> ToolKind {
        ToolKind::SubagentManagement
    }
    fn name(&self) -> &str {
        "cancel_task"
    }
    fn description(&self) -> &str {
        "Cancel a queued or running child task and wait for cancellation confirmation. Completed tasks stay completed. In-flight external tool effects may already have occurred; never assume cancellation undoes them."
    }
    fn parameters(&self) -> Value {
        serde_json::to_value(schema_for!(CancelTaskArgs)).expect("tool schema is serializable")
    }
    async fn execute(
        &self,
        args_json: &str,
        context: &mut ToolContext<'_>,
    ) -> anyhow::Result<String> {
        let args: CancelTaskArgs = serde_json::from_str(args_json)?;
        let scope = context
            .subagents
            .context("subagent manager is not configured")?;
        Ok(serde_json::to_string(&scope.cancel(&args.task_id).await?)?)
    }
    fn compress_output(
        &self,
        output: &str,
        budget: &ToolOutputBudget,
    ) -> anyhow::Result<CompressedToolOutput> {
        compress_task_json(output, budget)
    }
}
