use crate::{
    approval::ToolOperation,
    skills::SkillSnapshot,
    tools::{ActivityKind, Tool, ToolContext, ToolExecutionMode, ToolSource},
};

use std::sync::Arc;

use serde_json::{Value, json};

pub struct LoadSkillTool(pub Arc<SkillSnapshot>);
#[async_trait::async_trait]
impl Tool for LoadSkillTool {
    fn approval_requirement(&self, _: &Value) -> anyhow::Result<ToolOperation> {
        Ok(ToolOperation::Read)
    }

    fn name(&self) -> &str {
        "load_skill"
    }
    fn description(&self) -> &str {
        "Load an enabled skill's full instructions when its description matches the task. Skills are guidance, not permission grants. Repeated loads are deduplicated in the active context."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false})
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Host
    }
    fn activity_kind(&self) -> ActivityKind {
        ActivityKind::Think
    }
    fn source_for_arguments(&self, args: &Value) -> ToolSource {
        self.0.source(args)
    }
    async fn execute(&self, args: &str, _: &mut ToolContext<'_>) -> anyhow::Result<String> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Args {
            name: String,
        }
        let args: Args = serde_json::from_str(args)?;
        let snapshot = self.0.clone();
        tokio::task::spawn_blocking(move || {
            serde_json::to_string(&snapshot.load(&args.name)?).map_err(Into::into)
        })
        .await?
    }
}
