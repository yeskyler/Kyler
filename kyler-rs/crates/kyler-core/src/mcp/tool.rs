use crate::tools::{
    ActivityKind, CompressedToolOutput, Tool, ToolContext, ToolExecutionMode, ToolOutputBudget,
    ToolSource,
};

use std::{sync::Arc, time::Duration};

use anyhow::{Result, ensure};
use serde_json::Value;

use kyler_mcp::{Connection, ToolDescriptor, tool_alias};

pub struct McpTool {
    name: String,
    server: String,
    remote: Arc<ToolDescriptor>,
    connection: Arc<Connection>,
    timeout: u64,
    activity: ActivityKind,
    catalog_version: String,
}
impl McpTool {
    pub(super) fn new(
        server: &str,
        remote: Arc<ToolDescriptor>,
        connection: Arc<Connection>,
        timeout: u64,
        activity: &str,
        catalog_version: &str,
    ) -> Result<Self> {
        let name = tool_alias(server, &remote.name);
        let catalog_version = catalog_version.to_owned();
        ensure!(name.len() <= 64, "MCP tool name exceeds provider limit");
        Ok(Self {
            name,
            server: server.into(),
            remote,
            connection,
            catalog_version,
            timeout,
            activity: match activity {
                "explore" => ActivityKind::Explore,
                "edit" => ActivityKind::Edit,
                "search" => ActivityKind::Search,
                "think" => ActivityKind::Think,
                _ => ActivityKind::Execute,
            },
        })
    }
}
#[async_trait::async_trait]
impl Tool for McpTool {
    fn approval_identity(&self) -> String {
        format!(
            "{}:{}:{}",
            self.connection.approval_id(),
            self.name,
            serde_json::to_string(self.remote.as_ref()).unwrap_or_default()
        )
    }
    fn source(&self) -> ToolSource {
        ToolSource::Mcp {
            server_id: self.server.clone(),
            remote_name: self.remote.name.to_string(),
        }
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Mcp
    }
    fn activity_kind(&self) -> ActivityKind {
        self.activity
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        self.remote
            .description
            .as_deref()
            .unwrap_or("Call a configured MCP server tool")
    }
    fn parameters(&self) -> Value {
        self.remote.input_schema.clone()
    }
    async fn execute(&self, args_json: &str, _: &mut ToolContext<'_>) -> Result<String> {
        self.connection
            .call(
                &self.remote.name,
                &self.catalog_version,
                args_json,
                Duration::from_secs(self.timeout),
            )
            .await
    }
    fn compress_output(
        &self,
        output: &str,
        budget: &ToolOutputBudget,
    ) -> Result<CompressedToolOutput> {
        if output.len() <= budget.max_bytes {
            return Ok(CompressedToolOutput::unchanged(output));
        }
        let render = |size| {
            serde_json::json!({"truncated":true,"original_bytes":output.len(),"preview":crate::tools::text_prefix(output,size)}).to_string()
        };
        let content = crate::tools::fit_entries(output.len(), budget, render)?;
        CompressedToolOutput::compressed(output, content, budget)
    }
}
