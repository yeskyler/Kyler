//! Session execution lifecycle, cancellation and terminal checkpoints.
use super::{
    Agent,
    events::{AgentStreamEvent, StreamControl},
};
use crate::{
    attachments::image::UserImage,
    context::ExecutionStatus,
    session::{Session, store::acquire_session},
    subagent::TaskOwner,
};

use anyhow::{Context, Result};

use kyler_protocol::api::{attachments::AttachmentKind, permissions::ApprovalStatus};

impl Agent {
    pub(super) async fn execute_session(
        &self,
        session: &mut Session,
        input: &str,
        stream: Option<&StreamControl>,
    ) -> Result<String> {
        self.execute_session_with_images(session, input, Vec::new(), stream)
            .await
    }
    #[tracing::instrument(name="agent_execution", skip_all, fields(
        session_id=%session.session_id, execution_id=tracing::field::Empty, model=%self.model
    ))]
    pub(super) async fn execute_session_with_images(
        &self,
        session: &mut Session,
        input: &str,
        images: Vec<UserImage>,
        stream: Option<&StreamControl>,
    ) -> Result<String> {
        self.execute_turn(
            session,
            super::UserTurn {
                text: input.into(),
                images,
                ..Default::default()
            },
            stream,
        )
        .await
    }
    pub(super) async fn execute_turn(
        &self,
        session: &mut Session,
        turn: super::UserTurn,
        stream: Option<&StreamControl>,
    ) -> Result<String> {
        let agent = self.bind_model(None)?;
        agent.execute_bound_session(session, turn, stream).await
    }
    async fn execute_bound_session(
        &self,
        session: &mut Session,
        turn: super::UserTurn,
        stream: Option<&StreamControl>,
    ) -> Result<String> {
        self.tool_output_budget.validate()?;
        let _lease = acquire_session(&session.session_id)?;
        session.validate()?;
        match (&session.environment, &self.environment) {
            (Some(saved), Some(active)) => anyhow::ensure!(
                saved == active.session_environment(),
                "session environment differs from the host-authorized environment"
            ),
            (None, Some(active)) => {
                session.environment = Some(active.session_environment().clone())
            }
            (Some(_), None) => anyhow::bail!(
                "sandboxed session requires its host-authorized execution environment"
            ),
            (None, None) => {}
        }
        let _store_lease = if let Some(store) = &self.session_store {
            Some(store.ensure_current(session).await?)
        } else {
            None
        };
        // Reconcile a previous dropped run in this process before archiving it.
        if let (Some(manager), Some(execution)) = (&self.subagent_manager, &session.execution) {
            let owner = TaskOwner {
                session_id: session.session_id.clone(),
                execution_id: execution.execution_id.clone(),
            };
            let tasks = manager.finish_owner(&owner).await;
            session.record_subagent_tasks(&tasks);
        }
        session.recover_interrupted()?;
        if !turn.images.is_empty()
            || turn
                .attachments
                .iter()
                .any(|a| a.kind == AttachmentKind::Image)
        {
            self.validate_image_input()?;
        }
        if turn
            .attachments
            .iter()
            .any(|attachment| attachment.kind != AttachmentKind::Image)
        {
            self.validate_document_input()?;
        }
        if let (Some(previous), Some(current)) = (
            session
                .execution
                .as_ref()
                .and_then(|execution| execution.model_snapshot.as_ref()),
            self.model_info(),
        ) {
            if previous.profile != current.profile
                || previous.provider != current.provider
                || previous.model != current.model
                || previous.capabilities.tokenizer != current.capabilities.tokenizer
            {
                session.memory.estimation_scale = 1.0;
                session.memory.last_provider_input_tokens = 0;
            }
        }
        let continuation = if turn.resume {
            let previous = session.current_execution()?;
            anyhow::ensure!(
                matches!(
                    previous.status,
                    ExecutionStatus::Failed | ExecutionStatus::Cancelled
                ),
                "Only failed or cancelled tasks can be resumed"
            );
            Some((previous.execution_id.clone(), previous.plan.clone()))
        } else {
            None
        };
        let resume_prompt = "Continue the previous interrupted task using its existing plan and completed tool results. Do not restart the original task or repeat completed side effects. An interrupted tool may have an unknown outcome: inspect current state before deciding what remains. Unknown side effects require a new explicit user instruction before repetition.";
        session.begin_turn(
            if turn.resume {
                resume_prompt
            } else {
                &turn.text
            },
            turn.images,
            turn.attachments,
        )?;
        if let Some((id, plan)) = continuation {
            let execution = session.current_execution_mut()?;
            execution.resumed_from = Some(id);
            execution.plan = plan;
        }

        session.current_execution_mut()?.model = Some(self.model.clone());
        session.current_execution_mut()?.model_snapshot = self.model_info();
        session.current_execution_mut()?.permissions =
            self.approval.as_ref().map(|a| a.current_policy());
        tracing::Span::current().record(
            "execution_id",
            session.current_execution()?.execution_id.as_str(),
        );
        tracing::info!(max_steps = self.max_steps, "agent execution started");
        let scope_result = if let Some(manager) = &self.subagent_manager {
            manager
                .open_scope(
                    TaskOwner {
                        session_id: session.session_id.clone(),
                        execution_id: session.current_execution()?.execution_id.clone(),
                    },
                    self.clone()
                        .with_mcp_scope(self.mcp_scope.as_deref().unwrap_or(&session.session_id))
                        .child_executor(manager),
                )
                .map(Some)
        } else {
            Ok(None)
        };
        let scope = match scope_result {
            Ok(scope) => scope,
            Err(error) => {
                session.finish(
                    if stream.is_some_and(|s| s.is_cancelled()) {
                        ExecutionStatus::Cancelled
                    } else {
                        ExecutionStatus::Failed
                    },
                    None,
                    Some(format!("{error:#}")),
                )?;
                self.checkpoint(session).await?;
                return Err(error);
            }
        };
        let result = async {
            self.checkpoint(session).await?;
            if let Some(stream) = stream {
                stream
                    .emit(AgentStreamEvent::Started {
                        execution_id: session.current_execution()?.execution_id.clone(),
                        session_id: session.session_id.clone(),
                    })
                    .await?;
                tokio::select! {
                    biased;
                    _ = stream.cancelled() => anyhow::bail!("agent execution cancelled"),
                    result = self.run_loop(session, scope.as_ref(), Some(stream)) => result,
                }
            } else {
                self.run_loop(session, scope.as_ref(), None).await
            }
        }
        .await;
        if let Some(scope) = &scope {
            let tasks = scope.close().await;
            session.record_subagent_tasks(&tasks);
        }
        if let Some(context) = &self.approval {
            if context.session_id == session.session_id {
                for record in context
                    .manager
                    .list(&context.session_id)
                    .into_iter()
                    .filter(|r| r.run_id == context.run_id)
                {
                    let records = &mut session.current_execution_mut()?.approvals;
                    if let Some(existing) = records.iter_mut().find(|r| r.id == record.id) {
                        *existing = record;
                    } else {
                        records.push(record);
                    }
                }
            }
        }
        for approval in &mut session.current_execution_mut()?.approvals {
            if approval.status == ApprovalStatus::Pending {
                approval.status = ApprovalStatus::Cancelled;
                approval.reason = "Execution ended before approval completed.".into();
            }
        }
        match result {
            Ok(output) => {
                session.finish(ExecutionStatus::Completed, Some(output.clone()), None)?;
                self.checkpoint(session)
                    .await
                    .context("execution completed but final session checkpoint failed")?;
                tracing::info!(
                    steps = session.current_execution()?.current_step,
                    total_tokens = session.current_execution()?.usage.total_tokens,
                    output_bytes = output.len(),
                    "agent execution completed"
                );
                Ok(output)
            }
            Err(error) => {
                session.finish(
                    if stream.is_some_and(|s| s.is_cancelled()) {
                        ExecutionStatus::Cancelled
                    } else {
                        ExecutionStatus::Failed
                    },
                    None,
                    Some(format!("{error:#}")),
                )?;
                if let Err(save_error) = self.checkpoint(session).await {
                    return Err(
                        error.context(format!("failed to save failed execution: {save_error:#}"))
                    );
                }
                tracing::warn!(status=?session.current_execution()?.status,"agent execution stopped");
                Err(error)
            }
        }
    }
}
