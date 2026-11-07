use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpStatus {
    Disconnected,
    Connecting,
    Connected,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    #[serde(default)]
    pub removable: bool,
    pub id: String,
    pub transport: String,
    pub status: McpStatus,
    pub tools: Vec<McpToolInfo>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAction {
    Connect,
    Disconnect,
    Reconnect,
}

/// User-defined connector. Adding a definition never starts a process or connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateConnectorRequest {
    pub id: String,
    pub transport: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub bearer_token_env: Option<String>,
}
