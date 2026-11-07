//! Explicit user clarification; never an execution approval.
use crate::{
    approval::ToolOperation,
    questions::{QuestionManager, QuestionWaiter},
    tools::{Tool, ToolContext, ToolExecutionMode},
};
use anyhow::Result;
use kyler_protocol::api::questions::AskQuestionArgs;
use serde_json::{Value, json};
use std::sync::Arc;

pub struct AskQuestionTool {
    pub manager: Arc<QuestionManager>,
    pub waiter: Option<Arc<dyn QuestionWaiter>>,
}
#[async_trait::async_trait]
impl Tool for AskQuestionTool {
    fn name(&self) -> &str {
        "ask_question"
    }
    fn description(&self) -> &str {
        "Ask one to three blocking questions only when required user input cannot be found in context or tools. Supports single_select, multi_select and text; users can always write a custom answer. Use stable IDs and concise options. Never request credentials or execution approval. Waits asynchronously for the user. A dismissed request means no answer was given; do not infer consent or repeat the same question."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","additionalProperties":false,"required":["questions"],"properties":{"questions":{"type":"array","minItems":1,"maxItems":3,"items":{"type":"object","additionalProperties":false,"required":["id","question","type"],"properties":{"id":{"type":"string"},"question":{"type":"string"},"type":{"type":"string","enum":["single_select","multi_select","text"]},"options":{"type":"array","maxItems":8,"items":{"type":"object","additionalProperties":false,"required":["id","label"],"properties":{"id":{"type":"string"},"label":{"type":"string"},"recommended":{"type":"boolean"}}}}}}}}})
    }
    fn validate_arguments(&self, args: &Value) -> Result<()> {
        crate::questions::validate(&serde_json::from_value::<AskQuestionArgs>(args.clone())?)
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Host
    }
    fn approval_requirement(&self, _: &Value) -> Result<ToolOperation> {
        Ok(ToolOperation::State)
    }
    async fn execute(&self, args: &str, context: &mut ToolContext<'_>) -> Result<String> {
        let record = self
            .manager
            .ask(
                context.session_id,
                &context.execution.execution_id,
                context.tool_call_id,
                serde_json::from_str(args)?,
                self.waiter.as_deref(),
            )
            .await?;
        Ok(
            json!({"question_id":record.id,"status":record.status,"answers":record.answers})
                .to_string(),
        )
    }
}
