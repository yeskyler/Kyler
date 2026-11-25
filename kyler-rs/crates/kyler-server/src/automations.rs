//! Translate scheduled intent into the existing core thread/Op admission path.
use crate::{error::ApiError, runs::RunManager};

use std::sync::{Arc, Weak};

use anyhow::{Context, Result, ensure};
use async_trait::async_trait;

use kyler_core::{ExecutionStatus, Session};
use kyler_protocol::api::{
    automations::{ScheduledRun, ScheduledRunStatus as Status, TaskConfig},
    permissions::{PermissionMode, PermissionSelection},
    run::{CreateRunRequest, RunInfo, RunStatus},
    session::WorkspaceMode,
};
use kyler_scheduler::{
    Error,
    executor::{ExecutionRef, ExecutionState, ScheduledExecutor, StartOutcome},
};

pub(crate) struct AutomationExecutor {
    pub manager: Weak<RunManager>,
}
impl AutomationExecutor {
    fn manager(&self) -> Result<Arc<RunManager>> {
        self.manager
            .upgrade()
            .context("Execution service is unavailable")
    }
    async fn reference(&self, run: &ScheduledRun) -> Result<Option<String>> {
        if let Some(id) = &run.core_run_id {
            return Ok(Some(id.clone()));
        }
        let manager = self.manager()?;
        let registry = manager.registry.lock().await;
        Ok(registry
            .keys
            .get(&(run.session_id.clone(), run.id.clone()))
            .map(|(id, _)| id.clone()))
    }
    async fn admit(
        &self,
        run: &ScheduledRun,
    ) -> Result<kyler_protocol::api::run::RunAccepted, ApiError> {
        let manager = self.manager.upgrade().ok_or_else(ApiError::stopping)?;
        let mut config = run.snapshot.clone();
        self.validate(&mut config)
            .await
            .map_err(|error| ApiError::bad(&error.to_string()))?;
        // A changed symlink or model default cannot silently retarget a saved task.
        if config.cwd != run.snapshot.cwd || config.model_profile != run.snapshot.model_profile {
            return Err(ApiError::bad(
                "Saved task configuration changed; edit and save the task before running it.",
            ));
        }
        let session = match manager.store.load(&run.session_id).await {
            Ok(session) => session,
            Err(error) if error.to_string() == "session not found" => {
                let mut session = Session::new();
                session.session_id = run.session_id.clone();
                session.environment = manager.environment_for_create(Some(&config.cwd))?;
                if config.workspace_mode == WorkspaceMode::Isolated {
                    manager.allocate_session_directory(&mut session)?;
                }
                manager.validate_session_environment(&session)?;
                session.state.insert(
                    "automation".into(),
                    serde_json::json!({"task_id":run.task_id,"run_id":run.id}),
                );
                manager
                    .store
                    .save(&mut session)
                    .await
                    .map_err(ApiError::storage)?;
                session
            }
            Err(error) => return Err(ApiError::storage(error)),
        };
        let thread = manager.core_thread(&session).await?;
        thread
            .handle
            .model(Some(config.model_profile.clone()))
            .await
            .map_err(|error| ApiError::bad(&error.to_string()))?;
        if thread
            .handle
            .thinking(None)
            .await
            .map_err(|error| ApiError::bad(&error.to_string()))?
            .supported
        {
            thread
                .handle
                .thinking_level(Some(config.thinking))
                .await
                .map_err(|error| ApiError::bad(&error.to_string()))?;
        }
        thread
            .handle
            .permissions(Some(&PermissionSelection {
                mode: Some(config.permission),
                approval: None,
                confirm_full_access: config.permission == PermissionMode::FullAccess,
            }))
            .await
            .map_err(|error| ApiError::bad(&error.to_string()))?;
        // Reuse RunManager's durable idempotency manifest and existing Op submission.
        manager
            .start(
                run.session_id.clone(),
                CreateRunRequest {
                    command: None,
                    skills: Vec::new(),
                    input: config.prompt,
                    attachment_ids: Vec::new(),
                    resume: false,
                    images: Vec::new(),
                    client_request_id: Some(run.id.clone()),
                },
            )
            .await
    }
}
#[async_trait]
impl ScheduledExecutor for AutomationExecutor {
    async fn validate(&self, config: &mut TaskConfig) -> Result<()> {
        let manager = self.manager()?;
        let snapshot = manager
            .environment_for_create(Some(&config.cwd))
            .map_err(|_| {
                Error::Invalid(
                    "Workspace is outside the service's allowed roots or is unavailable.".into(),
                )
            })?
            .ok_or_else(|| Error::Invalid("Automation requires an execution workspace.".into()))?;
        config.cwd = snapshot.cwd.to_string_lossy().into_owned();
        let environment = if let Some(factory) = &manager.workspaces {
            Some(factory.create(&snapshot.cwd)?)
        } else {
            manager.agent.execution_environment()
        };
        let environment = environment.context("Execution environment is unavailable")?;
        ensure!(
            config.permission != PermissionMode::FullAccess || environment.allows_full_access(),
            Error::Invalid("Full access is unavailable in this execution environment.".into())
        );
        ensure!(
            config.permission != PermissionMode::WorkspaceAccess
                || !environment.policy().filesystem.readwrite_paths.is_empty(),
            Error::Invalid("This execution environment is read-only.".into())
        );
        let bound = manager
            .agent
            .bind_model(config.model_profile.as_deref())
            .map_err(|_| Error::Invalid("The selected model profile is unavailable.".into()))?;
        config.model_profile = Some(
            bound
                .model_info()
                .context("Model information is unavailable")?
                .profile,
        );
        ensure!(
            config.prompt.len() <= manager.settings.max_request_bytes,
            Error::Invalid("Task instructions exceed the service request limit.".into())
        );
        Ok(())
    }
    async fn start(&self, run: &ScheduledRun) -> StartOutcome {
        match self.admit(run).await {
            Ok(accepted)=>StartOutcome::Started(ExecutionRef { run_id:accepted.run_id,execution_id:None }),
            Err(error) if error.body.code=="capacity_exceeded" || error.body.code=="server_stopping"=>StartOutcome::Deferred,
            Err(error) if error.status.is_client_error()=>StartOutcome::Failed(error.body.message),
            Err(_)=>StartOutcome::Unknown("Admission could not be confirmed. Task paused; inspect the linked session before enabling it.".into()),
        }
    }
    async fn inspect(&self, run: &ScheduledRun) -> Result<Option<ExecutionState>> {
        let manager = self.manager()?;
        let Some(id) = self.reference(run).await? else {
            return Ok(None);
        };
        if let Ok(entry) = manager.entry(&id).await {
            let info = entry.info();
            return Ok(Some(state(&manager, &info)));
        }
        // Completed manifests may have expired; only match a captured execution ID.
        if let Some(execution_id) = &run.execution_id {
            let session = manager.store.load(&run.session_id).await?;
            if let Some(execution) = session
                .execution
                .iter()
                .chain(session.executions.iter())
                .find(|execution| &execution.execution_id == execution_id)
            {
                let status = match execution.status {
                    ExecutionStatus::Running => Status::Interrupted,
                    ExecutionStatus::Completed => Status::Completed,
                    ExecutionStatus::Failed => Status::Failed,
                    ExecutionStatus::Cancelled => Status::Cancelled,
                };
                return Ok(Some(ExecutionState {
                    status,
                    reference: Some(ExecutionRef {
                        run_id: id,
                        execution_id: Some(execution_id.clone()),
                    }),
                    summary: execution
                        .final_result
                        .as_ref()
                        .map(|text| text.chars().take(2048).collect()),
                    error: if status == Status::Interrupted {
                        Some("Execution was interrupted by a service restart.".into())
                    } else {
                        execution
                            .error
                            .as_ref()
                            .map(|_| "Execution did not complete successfully.".into())
                    },
                }));
            }
        }
        Ok(None)
    }
    async fn cancel(&self, run: &ScheduledRun) -> Result<()> {
        let manager = self.manager()?;
        if let Some(id) = self.reference(run).await? {
            manager
                .cancel(&id)
                .await
                .map_err(|_| anyhow::anyhow!("Cancellation could not be confirmed"))?;
        }
        Ok(())
    }
}
fn state(manager: &RunManager, info: &RunInfo) -> ExecutionState {
    let status = match info.status {
        RunStatus::Starting => Status::Starting,
        RunStatus::Running | RunStatus::Cancelling => {
            if manager
                .approvals
                .as_ref()
                .is_some_and(|approvals| approvals.waiting(&info.run_id))
            {
                Status::WaitingForApproval
            } else if manager.questions.waiting(&info.session_id) {
                Status::WaitingForInput
            } else {
                Status::Running
            }
        }
        RunStatus::Completed => Status::Completed,
        RunStatus::Cancelled => Status::Cancelled,
        RunStatus::Failed
            if info
                .error
                .as_ref()
                .is_some_and(|error| error.code == "server_restarted") =>
        {
            Status::Interrupted
        }
        RunStatus::Failed => Status::Failed,
    };
    ExecutionState {
        status,
        reference: Some(ExecutionRef {
            run_id: info.run_id.clone(),
            execution_id: info.execution_id.clone(),
        }),
        summary: info
            .output
            .as_ref()
            .map(|text| text.chars().take(2048).collect()),
        error: info.error.as_ref().map(|error| error.message.clone()),
    }
}
