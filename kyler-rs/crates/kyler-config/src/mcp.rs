use crate::SecretString;

use std::collections::{BTreeMap, HashMap};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpSettings {
    pub servers: BTreeMap<String, McpServerSettings>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpServerSettings {
    pub enabled: bool,
    pub auto_connect: bool,
    pub transport: McpTransport,
    pub command: String,
    pub args: Vec<String>,
    pub url: String,
    pub bearer_token_env: Option<String>,
    pub env: BTreeMap<String, String>,
    pub activity: String,
    #[serde(skip)]
    pub resolved_env: BTreeMap<String, SecretString>,
    #[serde(skip)]
    pub resolved_token: Option<SecretString>,
    pub connect_timeout_seconds: u64,
    pub call_timeout_seconds: u64,
}
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    #[default]
    Stdio,
    StreamableHttp,
}
impl Default for McpServerSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_connect: true,
            transport: McpTransport::Stdio,
            command: String::new(),
            args: Vec::new(),
            url: String::new(),
            bearer_token_env: None,
            env: BTreeMap::new(),
            activity: "execute".into(),
            resolved_env: BTreeMap::new(),
            resolved_token: None,
            connect_timeout_seconds: 10,
            call_timeout_seconds: 60,
        }
    }
}
impl McpSettings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.servers.len() <= 32,
            "at most 32 MCP servers are supported"
        );
        for (id, server) in &self.servers {
            ensure!(
                !id.is_empty()
                    && id.len() <= 24
                    && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'),
                "MCP server IDs must use 1..=24 ASCII letters, digits or underscores"
            );
            ensure!(
                (1..=60).contains(&server.connect_timeout_seconds)
                    && (1..=300).contains(&server.call_timeout_seconds),
                "MCP timeout is outside the supported range"
            );
            ensure!(
                matches!(
                    server.activity.as_str(),
                    "explore" | "execute" | "edit" | "search" | "think"
                ),
                "invalid MCP activity category"
            );
            if !server.enabled {
                continue;
            }
            match server.transport {
                McpTransport::Stdio => ensure!(
                    std::path::Path::new(&server.command).is_absolute(),
                    "MCP stdio command must be an absolute executable path"
                ),
                McpTransport::StreamableHttp => {
                    let url = url::Url::parse(&server.url)
                        .map_err(|_| anyhow::anyhow!("invalid MCP server URL"))?;
                    ensure!(
                        matches!(url.scheme(), "http" | "https")
                            && url.host_str().is_some()
                            && url.username().is_empty()
                            && url.password().is_none()
                            && url.query().is_none()
                            && url.fragment().is_none(),
                        "MCP URL must be HTTP(S) without credentials, query or fragment"
                    );
                }
            }
            ensure!(
                server.env.iter().all(|(key, reference)| !key.is_empty()
                    && !reference.is_empty()
                    && !key.contains(['=', '\0'])
                    && !reference.contains(['=', '\0'])),
                "MCP environment entries must map variable names to environment variable references"
            );
        }
        Ok(())
    }
}

impl McpSettings {
    /// Called after configuration and environment references have been merged.
    pub fn validate_credentials(&self) -> Result<()> {
        for (id, server) in &self.servers {
            if !server.enabled {
                continue;
            }
            for (key, reference) in &server.env {
                ensure!(
                    server
                        .resolved_env
                        .get(key)
                        .is_some_and(|value| !value.is_empty()),
                    "MCP server {id} requires environment reference {reference}; configure it before startup"
                );
            }
            if let Some(reference) = &server.bearer_token_env {
                ensure!(
                    server
                        .resolved_token
                        .as_ref()
                        .is_some_and(|value| !value.is_empty()),
                    "MCP server {id} requires environment reference {reference}; configure it before startup"
                );
            }
        }
        Ok(())
    }

    pub fn resolve_environment(&mut self, values: &HashMap<String, String>) {
        for server in self.servers.values_mut() {
            for (key, reference) in &server.env {
                if let Some(value) = values.get(reference) {
                    server
                        .resolved_env
                        .insert(key.clone(), SecretString::new(value));
                }
            }
            if let Some(value) = server
                .bearer_token_env
                .as_ref()
                .and_then(|name| values.get(name))
            {
                server.resolved_token = Some(SecretString::new(value));
            }
        }
    }
}
