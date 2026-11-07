//! Background delegation contracts and task metadata; child histories stay isolated.
pub mod manager;
pub(crate) mod review;

use crate::{context::TokenUsage, environment::SessionEnvironment, session::Session};

use anyhow::{Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use kyler_config::SubagentSettings;

pub use kyler_protocol::review::{AgentKind, ReviewRequest, ReviewResult};
pub use manager::{SubagentManager, SubagentScope};

#[derive(Debug, Clone)]
pub struct SubagentConfig {
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
impl Default for SubagentConfig {
    fn default() -> Self {
        Self::from(&SubagentSettings::default())
    }
}
impl From<&SubagentSettings> for SubagentConfig {
    fn from(s: &SubagentSettings) -> Self {
        Self {
            max_concurrent_tasks: s.max_concurrent_tasks,
            max_tasks_per_execution: s.max_tasks_per_execution,
            max_queued_tasks: s.max_queued_tasks,
            max_retained_tasks: s.max_retained_tasks,
            max_steps: s.max_steps,
            task_timeout_secs: s.task_timeout_secs,
            queue_timeout_secs: s.queue_timeout_secs,
            max_wait_secs: s.max_wait_secs,
            retention_secs: s.retention_secs,
        }
    }
}
impl SubagentConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.max_concurrent_tasks > 0
                && self.max_concurrent_tasks <= kyler_protocol::agents::MAX_CONCURRENT_SUBAGENTS,
            "subagent concurrency must be in 1..=32"
        );
        ensure!(
            self.max_tasks_per_execution > 0
                && self.max_tasks_per_execution
                    <= kyler_protocol::agents::MAX_SUBAGENTS_PER_EXECUTION
                && self.max_queued_tasks > 0
                && self.max_retained_tasks >= self.max_concurrent_tasks,
            "invalid subagent queue or retention limits"
        );
        ensure!(
            self.max_steps > 0
                && self.task_timeout_secs > 0
                && self.queue_timeout_secs > 0
                && self.max_wait_secs > 0
                && self.retention_secs > 0,
            "subagent step and time limits must be positive"
        );
        ensure!(
            [
                self.task_timeout_secs,
                self.queue_timeout_secs,
                self.max_wait_secs,
                self.retention_secs
            ]
            .iter()
            .all(|s| *s <= 86400 * 365),
            "subagent time limits cannot exceed one year"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskOwner {
    pub session_id: String,
    pub execution_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParentSessionRef {
    pub session_id: String,
    pub execution_id: String,
    pub tool_call_id: String,
    pub task_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Queued,
    Running,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
}
impl SubagentStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTask {
    #[serde(default)]
    pub author: crate::context::EventAuthor,
    #[serde(default)]
    pub agent: AgentKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewResult>,
    pub task_id: String,
    pub parent_session_id: String,
    pub parent_execution_id: String,
    pub parent_tool_call_id: String,
    pub session_id: String,
    pub execution_id: Option<String>,
    pub status: SubagentStatus,
    pub output: Option<String>,
    pub error: Option<String>,
    pub usage: TokenUsage,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DelegateTaskArgs {
    #[serde(default)]
    pub agent: AgentKind,
    #[serde(default)]
    pub review: Option<ReviewRequest>,
    pub task: String,
    pub context: Option<String>,
}
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaitMode {
    #[default]
    Any,
    All,
}
#[derive(Debug, Serialize)]
pub struct TaskWaitResult {
    pub tasks: Vec<SubagentTask>,
    pub timed_out: bool,
}

pub struct SubagentOutput {
    pub output: String,
    pub review: Option<ReviewResult>,
}

impl DelegateTaskArgs {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.task.trim().is_empty() && self.task.len() <= 32000,
            "Delegated task must contain 1 to 32000 bytes"
        );
        ensure!(
            self.context
                .as_ref()
                .is_none_or(|context| context.len() <= 128000),
            "Delegated context exceeds 128000 bytes"
        );
        match (self.agent, &self.review) {
            (AgentKind::General, None) => Ok(()),
            (AgentKind::Review, Some(request)) => review::validate_request(request),
            _ => anyhow::bail!(
                "Review agents require review.target; general agents cannot accept a review target"
            ),
        }
    }
}

/// The manager depends on an execution interface rather than recursively on Agent.
#[async_trait::async_trait]
pub trait SubagentExecutor: Send + Sync {
    fn for_task(
        &self,
        request: &DelegateTaskArgs,
    ) -> Result<Option<std::sync::Arc<dyn SubagentExecutor>>> {
        ensure!(
            request.agent == AgentKind::General,
            "This executor does not support review agents"
        );
        Ok(None)
    }
    async fn execute_result(&self, session: &mut Session, input: &str) -> Result<SubagentOutput> {
        Ok(SubagentOutput {
            output: self.execute(session, input).await?,
            review: None,
        })
    }

    fn waiting_for_approval(&self, _session: &str) -> bool {
        false
    }
    fn session_environment(&self) -> Option<SessionEnvironment> {
        None
    }
    async fn execute(&self, session: &mut Session, input: &str) -> Result<String>;
}

#[cfg(test)]
mod limit_tests {
    use super::SubagentConfig;
    #[test]
    fn limits_default_to_32_running_and_64_created_and_cannot_exceed_them() {
        let mut config = SubagentConfig::default();
        assert_eq!(config.max_concurrent_tasks, 32);
        assert_eq!(config.max_tasks_per_execution, 64);
        config.validate().unwrap();
        config.max_concurrent_tasks = 33;
        assert!(config.validate().is_err());
        config.max_concurrent_tasks = 32;
        config.max_tasks_per_execution = 65;
        assert!(config.validate().is_err());
    }
}
