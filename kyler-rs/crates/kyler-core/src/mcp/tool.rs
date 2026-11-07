use super::connection::Connection;
use crate::tools::{
    ActivityKind, CompressedToolOutput, Tool, ToolContext, ToolExecutionMode, ToolOutputBudget,
    ToolSource,
};

use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use anyhow::{Result, ensure};
use serde_json::Value;
use tokio::runtime::Handle;

pub(super) fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control),
        "invalid MCP tool name"
    );
    Ok(())
}
fn alias(server: &str, remote: &str) -> String {
    let simple = format!("mcp__{server}__{remote}");
    if simple.len() <= 64
        && remote
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return simple;
    }
    let hash = remote.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    let prefix: String = remote
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(40 - server.len())
        .collect();
    format!("mcp__{server}__{prefix}_{hash:016x}")
}
pub struct McpTool {
    name: String,
    server: String,
    remote: rmcp::model::Tool,
    connection: Arc<Connection>,
    timeout: u64,
    activity: ActivityKind,
}
impl McpTool {
    pub(super) fn new(
        server: &str,
        remote: rmcp::model::Tool,
        connection: Arc<Connection>,
        timeout: u64,
        activity: &str,
    ) -> Result<Self> {
        validate_name(&remote.name)?;
        let name = alias(server, &remote.name);
        ensure!(name.len() <= 64, "MCP tool name exceeds provider limit");
        Ok(Self {
            name,
            server: server.into(),
            remote,
            connection,
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
            self.connection.approval_id,
            self.name,
            serde_json::to_string(&self.remote).unwrap_or_default()
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
        Value::Object(self.remote.input_schema.as_ref().clone())
    }
    async fn execute(&self, args_json: &str, _: &mut ToolContext<'_>) -> Result<String> {
        ensure!(
            self.connection.enabled.load(Ordering::SeqCst) && !self.connection.service.is_closed(),
            "MCP connection is unavailable; reconnect before retrying"
        );
        self.connection.calls.fetch_add(1, Ordering::SeqCst);
        struct Lease<'a>(&'a Connection);
        impl Drop for Lease<'_> {
            fn drop(&mut self) {
                self.0.calls.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let _lease = Lease(&self.connection);
        ensure!(
            self.connection.enabled.load(Ordering::SeqCst),
            "MCP connection is disconnecting"
        );
        let args: serde_json::Map<String, Value> = serde_json::from_str(args_json)?;
        let params = rmcp::model::CallToolRequestParams::new(self.remote.name.to_string())
            .with_arguments(args);
        let handle = self
            .connection
            .service
            .send_request_with_option(
                rmcp::model::ClientRequest::CallToolRequest(rmcp::model::CallToolRequest::new(
                    params,
                )),
                rmcp::service::PeerRequestOptions::no_options(),
            )
            .await
            .map_err(|_| {
                self.connection.enabled.store(false, Ordering::SeqCst);
                anyhow::anyhow!("MCP tool request failed; it was not retried.")
            })?;
        struct CancelOnDrop(Option<rmcp::service::RequestHandle<rmcp::RoleClient>>);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                if let Some(handle) = self.0.take() {
                    if let Ok(runtime) = Handle::try_current() {
                        runtime.spawn(async move {
                            let _ = handle
                                .cancel(Some("Kyler tool call cancelled or timed out".into()))
                                .await;
                        });
                    }
                }
            }
        }
        let mut pending = CancelOnDrop(Some(handle));
        let response = tokio::time::timeout(
            Duration::from_secs(self.timeout),
            &mut pending.0.as_mut().unwrap().rx,
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "MCP tool timed out; remote side effects may have occurred. It was not retried."
            )
        })?
        .map_err(|_| {
            self.connection.enabled.store(false, Ordering::SeqCst);
            anyhow::anyhow!("MCP connection closed; remote outcome may be unknown.")
        })?
        .map_err(|error| {
            if matches!(
                error,
                rmcp::service::ServiceError::TransportSend(_)
                    | rmcp::service::ServiceError::TransportClosed
            ) {
                self.connection.enabled.store(false, Ordering::SeqCst);
            }
            anyhow::anyhow!("MCP request failed; it was not retried.")
        })?;
        pending.0.take();
        let rmcp::model::ServerResult::CallToolResult(result) = response else {
            anyhow::bail!("Unexpected MCP tool response");
        };
        // Preserve typed content and structuredContent; never silently discard non-text blocks.
        let output = serde_json::to_string(&result)?;
        ensure!(
            output.len() <= 4 * 1024 * 1024,
            "MCP tool result exceeds 4 MiB"
        );
        ensure!(
            !result.is_error.unwrap_or(false),
            "MCP tool returned an error: {output}"
        );
        Ok(output)
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
            serde_json::json!({"truncated":true,"original_bytes":output.len(),"preview":crate::tools::output::text_prefix(output,size)}).to_string()
        };
        let content = crate::tools::output::fit_entries(output.len(), budget, render)?;
        CompressedToolOutput::compressed(output, content, budget)
    }
}
