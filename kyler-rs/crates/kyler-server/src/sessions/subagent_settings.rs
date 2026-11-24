//! Persistent host configuration edits; running delegation scopes retain their limits.
use crate::{
    error::{ApiError, ApiResult},
    runs::RunManager,
};

use std::{fs, io::Write};

use anyhow::{Context, Result, ensure};

use kyler_config::loader;
use kyler_protocol::{
    agents::{MAX_CONCURRENT_SUBAGENTS, MAX_SUBAGENTS_PER_EXECUTION},
    api::subagents::{SubagentSettingsResponse, SubagentSettingsUpdate},
};

fn view(settings: &kyler_config::SubagentSettings) -> SubagentSettingsUpdate {
    SubagentSettingsUpdate {
        enabled: settings.enabled,
        max_concurrent_tasks: settings.max_concurrent_tasks,
        max_tasks_per_execution: settings.max_tasks_per_execution,
    }
}
impl RunManager {
    pub async fn subagent_settings(&self) -> ApiResult<SubagentSettingsResponse> {
        let source = self
            .model_source
            .read()
            .map_err(|_| ApiError::internal())?
            .clone()
            .ok_or_else(|| ApiError::bad("This server has no editable configuration source."))?;
        let config = tokio::task::spawn_blocking(move || {
            loader::load_with_llm_overrides(Some(&source.0), &source.1)
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::bad("Cannot load subagent configuration."))?;
        let configured = view(&config.subagent);
        let active = match self.agent.subagent_manager() {
            Some(manager) => SubagentSettingsUpdate {
                enabled: true,
                max_concurrent_tasks: manager.config().max_concurrent_tasks,
                max_tasks_per_execution: manager.config().max_tasks_per_execution,
            },
            None => SubagentSettingsUpdate {
                enabled: false,
                max_concurrent_tasks: configured.max_concurrent_tasks,
                max_tasks_per_execution: configured.max_tasks_per_execution,
            },
        };
        let restart_required =
            active.enabled != configured.enabled || (active.enabled && active != configured);
        Ok(SubagentSettingsResponse {
            configured,
            active,
            restart_required,
            max_concurrent_limit: MAX_CONCURRENT_SUBAGENTS,
            max_tasks_limit: MAX_SUBAGENTS_PER_EXECUTION,
        })
    }
    pub async fn update_subagent_settings(
        &self,
        request: SubagentSettingsUpdate,
    ) -> ApiResult<SubagentSettingsResponse> {
        if !(1..=MAX_CONCURRENT_SUBAGENTS).contains(&request.max_concurrent_tasks)
            || !(1..=MAX_SUBAGENTS_PER_EXECUTION).contains(&request.max_tasks_per_execution)
        {
            return Err(ApiError::bad(
                "Subagent concurrency must be in 1..=32 and tasks per execution in 1..=64.",
            ));
        }
        // Serialize with provider/model edits to prevent lost configuration changes.
        let _edit = self.model_reload.lock().await;
        let source = self
            .model_source
            .read()
            .map_err(|_| ApiError::internal())?
            .clone()
            .ok_or_else(|| ApiError::bad("This server has no editable configuration source."))?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let path = fs::canonicalize(&source.0)?;
            let text = fs::read_to_string(&path)?;
            let mut values: toml::Table = toml::from_str(&text)?;
            let settings = values.entry("subagent").or_insert_with(|| toml::Value::Table(Default::default())).as_table_mut().context("Invalid subagent table")?;
            settings.insert("enabled".into(), request.enabled.into());
            settings.insert("max_concurrent_tasks".into(), (request.max_concurrent_tasks as i64).into());
            settings.insert("max_tasks_per_execution".into(), (request.max_tasks_per_execution as i64).into());
            let directory = path.parent().context("Configuration directory unavailable")?;
            let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
            temporary.write_all(toml::to_string_pretty(&values)?.as_bytes())?;
            temporary.as_file().sync_all()?;
            let candidate = loader::load_with_llm_overrides(Some(temporary.path()), &source.1)?;
            ensure!(view(&candidate.subagent) == request, "Startup overrides control subagent settings");
            ensure!(fs::read_to_string(&path)? == text, "Configuration changed while saving");
            temporary.persist(&path).map_err(|_| anyhow::anyhow!("Cannot save configuration"))?;
            fs::File::open(directory)?.sync_all()?;
            Ok(())
        }).await.map_err(|_| ApiError::internal())?.map_err(|_| ApiError::bad("Cannot save subagent settings. Check concurrency and retention limits. Disable subagent session persistence in the server configuration before turning off delegation."))?;
        self.subagent_settings().await
    }
}
