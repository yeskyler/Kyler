use crate::{
    approval::ToolOperation,
    skills::SkillSnapshot,
    tools::{
        ActivityKind, CompressedToolOutput, Tool, ToolContext, ToolExecutionMode, ToolOutputBudget,
        ToolSource,
        output::{fit_entries, text_prefix},
    },
};

use std::sync::Arc;

use serde_json::{Value, json};

pub struct SkillsResourceTool(pub Arc<SkillSnapshot>);
#[async_trait::async_trait]
impl Tool for SkillsResourceTool {
    fn approval_requirement(&self, _: &Value) -> anyhow::Result<ToolOperation> {
        Ok(ToolOperation::Read)
    }

    fn name(&self) -> &str {
        "skills_resource"
    }
    fn description(&self) -> &str {
        "Read a UTF-8 resource from an enabled skill using its package-relative path. Scripts are returned as text and are never executed. Absolute paths, traversal and symlinks are rejected."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"name":{"type":"string"},"path":{"type":"string"}},"required":["name","path"],"additionalProperties":false})
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Host
    }
    fn activity_kind(&self) -> ActivityKind {
        ActivityKind::Explore
    }
    fn source_for_arguments(&self, args: &Value) -> ToolSource {
        self.0.source(args)
    }
    fn compress_output(
        &self,
        output: &str,
        budget: &ToolOutputBudget,
    ) -> anyhow::Result<CompressedToolOutput> {
        if output.len() <= budget.max_bytes {
            return Ok(CompressedToolOutput::unchanged(output));
        }
        let value: Value = serde_json::from_str(output)?;
        let content = value["content"].as_str().unwrap_or_default();
        let render = |size| {
            json!({"skill":value["skill"],"fingerprint":value["fingerprint"],"path":value["path"],"truncated":true,"original_bytes":content.len(),"content":text_prefix(content,size)}).to_string()
        };
        let compact = fit_entries(content.len(), budget, render)?;
        CompressedToolOutput::compressed(output, compact, budget)
    }
    async fn execute(&self, args: &str, _: &mut ToolContext<'_>) -> anyhow::Result<String> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Args {
            name: String,
            path: String,
        }
        let args: Args = serde_json::from_str(args)?;
        let snapshot = self.0.clone();
        tokio::task::spawn_blocking(move || {
            Ok(snapshot.resource(&args.name, &args.path)?.to_string())
        })
        .await?
    }
}
