use crate::{
    approval::ToolOperation,
    attachments::AttachmentStore,
    tools::{
        ActivityKind, ParallelToolContext, Tool, ToolConcurrency, ToolContext, ToolExecutionMode,
    },
};

use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

use kyler_protocol::api::attachments::{AttachmentKind, ReadAttachmentRequest};

pub struct ReadAttachmentTool(pub Arc<AttachmentStore>);
pub struct ViewImageTool(pub Arc<AttachmentStore>);

#[async_trait::async_trait]
impl Tool for ReadAttachmentTool {
    fn name(&self) -> &str {
        "read_attachment"
    }
    fn description(&self) -> &str {
        "Read a non-image attachment in the current chat. Provide only attachment_id for an overview with type and unit count, or provide unit (zero-based page/sheet), offset or limit to read document text. Use view_image for images."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"attachment_id":{"type":"string"},"unit":{"type":"integer","minimum":0},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":500}},"required":["attachment_id"],"additionalProperties":false})
    }
    fn approval_requirement(&self, _: &Value) -> Result<ToolOperation> {
        Ok(ToolOperation::Read)
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Host
    }
    fn activity_kind(&self) -> ActivityKind {
        ActivityKind::Explore
    }
    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::Parallel
    }
    async fn execute(&self, args: &str, context: &mut ToolContext<'_>) -> Result<String> {
        self.execute_parallel(args, &context.parallel()).await
    }
    async fn execute_parallel(
        &self,
        args: &str,
        context: &ParallelToolContext<'_>,
    ) -> Result<String> {
        let value: Value = serde_json::from_str(args)?;
        let overview = ["unit", "offset", "limit"]
            .iter()
            .all(|key| value.get(key).is_none());
        let mut request: ReadAttachmentRequest = serde_json::from_value(value)?;
        let environment = context
            .environment
            .context("Document parsing requires an isolated execution environment")?;
        if overview {
            request.limit = Some(1);
        }
        let content = self
            .0
            .read_isolated(context.session_id, &request, environment)
            .await?;
        if overview {
            Ok(json!({"info":content.info,"units":content.units,"first_unit":content.unit_name,"first_unit_lines":content.total_lines}).to_string())
        } else {
            Ok(serde_json::to_string(&content)?)
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewImageArgs {
    attachment_id: String,
}

#[async_trait::async_trait]
impl Tool for ViewImageTool {
    fn name(&self) -> &str {
        "view_image"
    }
    fn description(&self) -> &str {
        "View an image from the current chat by attachment_id. The runtime supplies the image as a visual input after the tool results. This tool accepts no filesystem paths."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"attachment_id":{"type":"string"}},"required":["attachment_id"],"additionalProperties":false})
    }
    fn approval_requirement(&self, _: &Value) -> Result<ToolOperation> {
        Ok(ToolOperation::Read)
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Host
    }
    fn activity_kind(&self) -> ActivityKind {
        ActivityKind::Explore
    }
    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::Parallel
    }
    async fn execute(&self, args: &str, context: &mut ToolContext<'_>) -> Result<String> {
        self.execute_parallel(args, &context.parallel()).await
    }
    async fn execute_parallel(
        &self,
        args: &str,
        context: &ParallelToolContext<'_>,
    ) -> Result<String> {
        let args: ViewImageArgs = serde_json::from_str(args)?;
        let store = self.0.clone();
        let session = context.session_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let info = store.get(&session, &args.attachment_id)?;
            ensure!(info.kind == AttachmentKind::Image, "Attachment is not an image");
            let image = store.image(&session, &info.id)?;
            let format = image.data_url.split_once(';').and_then(|(prefix, _)| prefix.strip_prefix("data:"));
            Ok(json!({"attachment_id":info.id,"name":info.name,"width":image.width,"height":image.height,"mime_type":format}).to_string())
        }).await?
    }
}
