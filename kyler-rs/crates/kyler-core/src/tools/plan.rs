use crate::{
    approval::ToolOperation,
    context::{ExecutionContext, ExecutionPlan, PlanStep, PlanStepStatus},
    tools::{Tool, ToolContext, ToolExecutionMode},
};

use anyhow::Context;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct UpdatePlanArgs {
    pub summary: String,
    pub steps: Vec<PlanStep>,
}

#[derive(Debug, Serialize)]
struct UpdatePlanOutput<'a> {
    summary: &'a str,
    step_count: usize,
    in_progress_step_id: Option<&'a str>,
}

pub fn update_plan(args_json: &str, context: &mut ExecutionContext) -> anyhow::Result<String> {
    let plan: ExecutionPlan = serde_json::from_str::<UpdatePlanArgs>(args_json)
        .context("failed to parse update_plan arguments")?
        .into();

    let in_progress_step_id = plan
        .steps
        .iter()
        .find(|step| step.status == PlanStepStatus::InProgress)
        .map(|step| step.id.as_str());

    context.plan = Some(plan.clone());

    serde_json::to_string(&UpdatePlanOutput {
        summary: &plan.summary,
        step_count: plan.steps.len(),
        in_progress_step_id,
    })
    .context("failed to serialize update_plan result")
}

impl From<UpdatePlanArgs> for ExecutionPlan {
    fn from(args: UpdatePlanArgs) -> Self {
        Self {
            summary: args.summary,
            steps: args.steps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::update_plan;
    use crate::context::{ExecutionContext, PlanStepStatus};

    #[test]
    fn stores_valid_plan_in_execution_context() {
        let mut context = ExecutionContext::new();
        let result = update_plan(
            r#"{
                "summary": "Research a topic",
                "steps": [
                    {"id": "research", "content": "Collect sources", "status": "in_progress"},
                    {"id": "answer", "content": "Write the answer", "status": "pending"}
                ]
            }"#,
            &mut context,
        )
        .expect("plan should be valid");

        assert!(result.contains("research"));

        let plan = context.plan.as_ref().expect("plan should be stored");

        assert_eq!(plan.steps[0].status, PlanStepStatus::InProgress);
    }
}

pub struct UpdatePlanTool;

#[async_trait::async_trait]
impl Tool for UpdatePlanTool {
    fn approval_requirement(&self, _: &Value) -> anyhow::Result<ToolOperation> {
        Ok(ToolOperation::State)
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Host
    }
    fn name(&self) -> &str {
        "update_plan"
    }

    fn description(&self) -> &str {
        "Replace the current execution plan for a multi-step task. Keep completed steps \
         completed and mark at most one current step as in_progress."
    }

    fn parameters(&self) -> Value {
        serde_json::to_value(schema_for!(UpdatePlanArgs))
            .expect("UpdatePlanArgs schema is always serializable")
    }

    async fn execute(
        &self,
        args_json: &str,
        context: &mut ToolContext<'_>,
    ) -> anyhow::Result<String> {
        update_plan(args_json, context.execution)
    }
}
