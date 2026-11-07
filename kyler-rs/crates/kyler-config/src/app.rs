use crate::{ModelSettings, SandboxSettings, SkillSettings, WorkspaceSettings, mcp::McpSettings};

use std::{
    fmt::{Debug, Formatter},
    net::SocketAddr,
    path::PathBuf,
};

use anyhow::ensure;
use serde::Deserialize;

use kyler_protocol::{api::models::ModelCapabilities, approval::ApprovalSettings};

pub use kyler_logging::LoggingConfig;

/// Compatibility name; there is only one logging configuration definition.
pub type LoggingSettings = LoggingConfig;

/// No Serialize/Display implementation: credentials are not configuration output.
#[derive(Clone, Default, Deserialize)]
#[serde(transparent)]
pub struct SecretString(String);
impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}
impl Debug for SecretString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub web: crate::WebSettings,
    pub workspace: WorkspaceSettings,
    pub sandbox: SandboxSettings,
    pub approval: ApprovalSettings,
    pub models: ModelSettings,
    pub skills: SkillSettings,
    pub client: ClientSettings,
    pub server: ServerSettings,
    pub llm: LlmSettings,
    pub agent: AgentSettings,
    pub context: ContextSettings,
    pub session: SessionSettings,
    pub subagent: SubagentSettings,
    pub logging: LoggingConfig,
    pub mcp: McpSettings,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LlmSettings {
    pub capabilities: ModelCapabilities,
    pub base_url: String,
    pub model: String,
    pub api_key: SecretString,
}
impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            capabilities: Default::default(),
            base_url: String::new(),
            model: String::new(),
            api_key: SecretString::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentSettings {
    pub max_steps: u32,
    pub additional_instructions: Option<String>,
    pub tool_output_max_bytes: usize,
}
impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            max_steps: 50,
            additional_instructions: None,
            tool_output_max_bytes: 8192,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContextSettings {
    pub context_tokens: usize,
    pub window_ratio: f64,
    pub trigger_ratio: f64,
    pub output_tokens: u32,
    pub safety_tokens: usize,
    pub summary_tokens: usize,
}
impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            context_tokens: 128_000,
            window_ratio: 0.6,
            trigger_ratio: 0.8,
            output_tokens: 8192,
            safety_tokens: 1024,
            summary_tokens: 4096,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionSettings {
    pub enabled: bool,
    pub directory: PathBuf,
}
impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            directory: PathBuf::from("sessions"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SubagentSettings {
    pub enabled: bool,
    pub persist_sessions: bool,
    pub max_concurrent_tasks: usize,
    pub max_tasks_per_execution: usize,
    pub max_queued_tasks: usize,
    pub max_retained_tasks: usize,
    pub max_steps: u32,
    pub task_timeout_secs: u64,
    pub queue_timeout_secs: u64,
    pub max_wait_secs: u64,
    pub retention_secs: u64,
}
impl Default for SubagentSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            persist_sessions: false,
            max_concurrent_tasks: kyler_protocol::agents::MAX_CONCURRENT_SUBAGENTS,
            max_tasks_per_execution: kyler_protocol::agents::MAX_SUBAGENTS_PER_EXECUTION,
            max_queued_tasks: 32,
            max_retained_tasks: 256,
            max_steps: 25,
            task_timeout_secs: 120,
            queue_timeout_secs: 30,
            max_wait_secs: 30,
            retention_secs: 3600,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerSettings {
    pub command_timeout_ms: u32,
    pub background_command_timeout_ms: u32,
    pub approval_timeout_seconds: u64,
    pub bind: SocketAddr,
    pub access_token: SecretString,
    pub allowed_origins: Vec<String>,
    pub max_concurrent_runs: usize,
    pub max_retained_runs: usize,
    pub max_subscribers_per_run: usize,
    pub max_request_bytes: usize,
    pub run_timeout_seconds: u64,
    pub event_buffer_max_events: usize,
    pub event_buffer_max_bytes: usize,
    pub completed_run_retention_seconds: u64,
    pub shutdown_grace_seconds: u64,
}
impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            command_timeout_ms: 30000,
            background_command_timeout_ms: 600000,
            approval_timeout_seconds: 300,
            bind: SocketAddr::from(([127, 0, 0, 1], 30001)),
            access_token: SecretString::default(),
            allowed_origins: Vec::new(),
            max_concurrent_runs: 8,
            max_retained_runs: 128,
            max_subscribers_per_run: 16,
            max_request_bytes: 65536,
            run_timeout_seconds: 600,
            event_buffer_max_events: 2048,
            event_buffer_max_bytes: 8388608,
            completed_run_retention_seconds: 1800,
            shutdown_grace_seconds: 30,
        }
    }
}
impl ServerSettings {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=3600000).contains(&self.command_timeout_ms)
                && (1..=3600000).contains(&self.background_command_timeout_ms),
            "Server command timeouts must be in 1..=3600000 ms"
        );
        anyhow::ensure!(
            (1..=86400).contains(&self.approval_timeout_seconds),
            "server.approval_timeout_seconds must be in 1..=86400"
        );
        ensure!(
            self.bind.ip().is_loopback() || !self.access_token.is_empty(),
            "non-loopback server binding requires server.access_token"
        );
        ensure!(
            self.access_token
                .expose()
                .bytes()
                .all(|b| b.is_ascii_graphic()),
            "server.access_token must contain only visible ASCII without spaces"
        );
        ensure!(
            (1..=1024).contains(&self.max_concurrent_runs)
                && self.max_retained_runs >= self.max_concurrent_runs
                && self.max_retained_runs <= 10000,
            "invalid server run limits"
        );
        ensure!(
            (1..=1024).contains(&self.max_subscribers_per_run),
            "invalid server subscriber limit"
        );
        ensure!(
            (1024..=64 * 1024 * 1024).contains(&self.max_request_bytes),
            "invalid server request size limit"
        );
        ensure!(
            (1..=100000).contains(&self.event_buffer_max_events)
                && (4096..=64 * 1024 * 1024).contains(&self.event_buffer_max_bytes),
            "invalid server event cache limits"
        );
        ensure!(
            (1..=86400).contains(&self.run_timeout_seconds)
                && (1..=604800).contains(&self.completed_run_retention_seconds)
                && (1..=300).contains(&self.shutdown_grace_seconds),
            "invalid server timeout or retention"
        );
        for origin in &self.allowed_origins {
            let parsed = url::Url::parse(origin)
                .map_err(|_| anyhow::anyhow!("invalid server.allowed_origins"))?;
            ensure!(
                matches!(parsed.scheme(), "http" | "https")
                    && parsed.host_str().is_some()
                    && parsed.username().is_empty()
                    && parsed.password().is_none()
                    && parsed.query().is_none()
                    && parsed.fragment().is_none()
                    && parsed.path() == "/"
                    && parsed.origin().ascii_serialization() == *origin,
                "allowed origins must be explicit HTTP(S) origins without paths"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClientSettings {
    pub server_url: String,
    pub access_token: SecretString,
    pub connect_timeout_seconds: u64,
    pub request_timeout_seconds: u64,
    pub stream_idle_timeout_seconds: u64,
    pub reconnect_max_attempts: u32,
    pub reconnect_delay_ms: u64,
    pub cancel_timeout_seconds: u64,
    pub max_event_bytes: usize,
    pub max_response_bytes: usize,
}
impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            server_url: "http://127.0.0.1:30001".into(),
            access_token: SecretString::default(),
            connect_timeout_seconds: 10,
            request_timeout_seconds: 30,
            stream_idle_timeout_seconds: 60,
            reconnect_max_attempts: 5,
            reconnect_delay_ms: 500,
            cancel_timeout_seconds: 30,
            max_event_bytes: 4 * 1024 * 1024,
            max_response_bytes: 16 * 1024 * 1024,
        }
    }
}
impl ClientSettings {
    pub fn validate(&self) -> anyhow::Result<()> {
        let url = url::Url::parse(&self.server_url)
            .map_err(|_| anyhow::anyhow!("client.server_url must be an absolute HTTP(S) URL"))?;
        ensure!(
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "client.server_url must be HTTP(S) without credentials, a query or a fragment"
        );
        ensure!(
            self.access_token
                .expose()
                .bytes()
                .all(|b| b.is_ascii_graphic()),
            "client.access_token must contain visible ASCII without spaces"
        );
        ensure!(
            [
                self.connect_timeout_seconds,
                self.request_timeout_seconds,
                self.stream_idle_timeout_seconds,
                self.cancel_timeout_seconds
            ]
            .into_iter()
            .all(|v| (1..=3600).contains(&v)),
            "invalid client timeout"
        );
        ensure!(
            self.reconnect_max_attempts <= 100 && (1..=30000).contains(&self.reconnect_delay_ms),
            "invalid client reconnection settings"
        );
        ensure!(
            (4096..=64 * 1024 * 1024).contains(&self.max_event_bytes)
                && (4096..=256 * 1024 * 1024).contains(&self.max_response_bytes),
            "invalid client response limits"
        );
        Ok(())
    }
}
