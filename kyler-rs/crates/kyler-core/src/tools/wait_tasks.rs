use crate::{
    approval::ToolOperation,
    subagent::WaitMode,
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
pub struct WaitTasksArgs {
    pub task_ids: Vec<String>,
    #[serde(default)]
    pub mode: WaitMode,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}
fn default_timeout() -> u64 {
    20
}

pub struct WaitTasksTool;
#[async_trait::async_trait]
impl Tool for WaitTasksTool {
    fn approval_requirement(&self, _: &Value) -> anyhow::Result<ToolOperation> {
        Ok(ToolOperation::State)
    }

    fn kind(&self) -> ToolKind {
        ToolKind::SubagentManagement
    }
    fn name(&self) -> &str {
        "wait_tasks"
    }
    fn description(&self) -> &str {
        "Wait for any or all specified child tasks, then return their current states and results. Timeout ends only this wait; tasks continue. Access is restricted to this parent execution. Collect all required results before the parent finishes."
    }
    fn parameters(&self) -> Value {
        serde_json::to_value(schema_for!(WaitTasksArgs)).expect("tool schema is serializable")
    }
    async fn execute(
        &self,
        args_json: &str,
        context: &mut ToolContext<'_>,
    ) -> anyhow::Result<String> {
        let args: WaitTasksArgs = serde_json::from_str(args_json)?;
        let scope = context
            .subagents
            .context("subagent manager is not configured")?;
        Ok(serde_json::to_string(
            &scope
                .wait(&args.task_ids, args.mode, args.timeout_secs)
                .await?,
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
