//! Session MCP connection management requests.
use super::{Client, valid_id};
use crate::error::{Error, Result};

use reqwest::Method;

use kyler_protocol::api::mcp::{McpAction, McpServerInfo};

impl Client {
    pub async fn mcp_servers(&self, session: &str) -> Result<Vec<McpServerInfo>> {
        valid_id(session)?;
        self.json(self.request(Method::GET, &format!("/v1/sessions/{session}/mcp")))
            .await
    }
    pub async fn mcp_action(&self, session: &str, server: &str, action: McpAction) -> Result<()> {
        valid_id(session)?;
        if server.is_empty()
            || !server
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            return Err(Error::invalid("Invalid MCP server ID."));
        }
        let action = match action {
            McpAction::Connect => "connect",
            McpAction::Disconnect => "disconnect",
            McpAction::Reconnect => "reconnect",
        };
        let response = self
            .request(
                Method::POST,
                &format!("/v1/sessions/{session}/mcp/{server}/{action}"),
            )
            .send()
            .await
            .map_err(|_| Error::transport())?;
        self.checked(response).await?;
        Ok(())
    }
}
