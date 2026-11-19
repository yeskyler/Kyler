//! Remote calls are never automatically replayed after an uncertain outcome.
use crate::Connection;

use std::{sync::atomic::Ordering, time::Duration};

use anyhow::{Result, ensure};
use serde_json::Value;
use tokio::runtime::Handle;

impl Connection {
    pub async fn call(
        &self,
        name: &str,
        catalog_version: &str,
        args_json: &str,
        timeout: Duration,
    ) -> Result<String> {
        ensure!(
            self.enabled.load(Ordering::SeqCst) && !self.service.is_closed(),
            "MCP connection is unavailable; reconnect before retrying"
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        struct Lease<'a>(&'a Connection);
        impl Drop for Lease<'_> {
            fn drop(&mut self) {
                self.0.calls.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let _lease = Lease(self);
        ensure!(
            self.enabled.load(Ordering::SeqCst),
            "MCP connection is disconnecting"
        );
        ensure!(
            self.catalog().version == catalog_version,
            "MCP tool catalog changed; refresh tools before calling"
        );
        ensure!(
            self.catalog().tools.iter().any(|tool| tool.name == name),
            "MCP tool is no longer available"
        );
        let args: serde_json::Map<String, Value> = serde_json::from_str(args_json)?;
        let params = rmcp::model::CallToolRequestParams::new(name.to_owned()).with_arguments(args);
        let handle = self
            .service
            .send_request_with_option(
                rmcp::model::ClientRequest::CallToolRequest(rmcp::model::CallToolRequest::new(
                    params,
                )),
                rmcp::service::PeerRequestOptions::no_options(),
            )
            .await
            .map_err(|_| {
                self.enabled.store(false, Ordering::SeqCst);
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
        let response = tokio::time::timeout(timeout, &mut pending.0.as_mut().unwrap().rx)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "MCP tool timed out; remote side effects may have occurred. It was not retried."
                )
            })?
            .map_err(|_| {
                self.enabled.store(false, Ordering::SeqCst);
                anyhow::anyhow!("MCP connection closed; remote outcome may be unknown.")
            })?
            .map_err(|error| {
                if matches!(
                    error,
                    rmcp::service::ServiceError::TransportSend(_)
                        | rmcp::service::ServiceError::TransportClosed
                ) {
                    self.enabled.store(false, Ordering::SeqCst);
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
}
