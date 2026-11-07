use crate::app::AppConfig;

use anyhow::{Result, ensure};

impl AppConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.workspace.default_directory.as_os_str().is_empty(),
            "workspace.default_directory cannot be empty"
        );
        self.sandbox.validate()?;
        if let Some(error) = self.approval.validation_error() {
            anyhow::bail!(error);
        }
        self.mcp.validate()?;
        self.mcp.validate_credentials()?;
        self.skills.validate()?;
        self.effective_models().validate()?;
        ensure!(self.agent.max_steps > 0, "agent.max_steps must be positive");
        ensure!(
            (1..=64 * 1024 * 1024).contains(&self.agent.tool_output_max_bytes),
            "agent.tool_output_max_bytes must be in 1..=67108864"
        );
        let c = &self.context;
        ensure!(
            c.window_ratio > 0.0 && c.window_ratio < c.trigger_ratio && c.trigger_ratio < 1.0,
            "context ratios must satisfy 0 < window < trigger < 1"
        );
        ensure!(
            c.output_tokens > 0 && c.summary_tokens > 0 && c.summary_tokens <= u32::MAX as usize,
            "context output and summary budgets must be positive and fit u32"
        );
        let reserved = (c.output_tokens as usize)
            .checked_add(c.safety_tokens)
            .and_then(|n| {
                c.summary_tokens
                    .checked_mul(2)
                    .and_then(|s| n.checked_add(s))
            });
        ensure!(
            reserved.is_some_and(|n| n < c.context_tokens),
            "context budget cannot fit output, summary and safety reservations"
        );
        if self.session.enabled {
            ensure!(
                !self.session.directory.as_os_str().is_empty(),
                "session.directory cannot be empty"
            );
        }
        let s = &self.subagent;
        ensure!(
            s.max_concurrent_tasks > 0
                && s.max_concurrent_tasks <= kyler_protocol::agents::MAX_CONCURRENT_SUBAGENTS
                && s.max_tasks_per_execution > 0
                && s.max_tasks_per_execution <= kyler_protocol::agents::MAX_SUBAGENTS_PER_EXECUTION
                && s.max_queued_tasks > 0
                && s.max_retained_tasks >= s.max_concurrent_tasks,
            "invalid subagent concurrency or queue limits"
        );
        ensure!(
            s.max_steps > 0
                && [
                    s.task_timeout_secs,
                    s.queue_timeout_secs,
                    s.max_wait_secs,
                    s.retention_secs
                ]
                .into_iter()
                .all(|v| (1..=86400 * 365).contains(&v)),
            "invalid subagent step or timeout limits"
        );
        ensure!(
            !s.persist_sessions || (s.enabled && self.session.enabled),
            "subagent.persist_sessions requires enabled subagent and session storage"
        );
        self.client.validate()?;
        self.server.validate()?;
        self.web.validate(self.server.bind)?;
        self.logging.validate()?;
        Ok(())
    }
}
