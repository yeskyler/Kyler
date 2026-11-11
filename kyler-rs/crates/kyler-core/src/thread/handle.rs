//! Thread submission, urgent controls, event ownership, and read-only queries.
use super::{HostedTurn, ThreadEvent, ThreadHandle, ThreadOp, projection};
use crate::{
    Agent, Session, approval::ApprovalManager, attachments::AttachmentStore,
    questions::QuestionManager, runtime::UserTurn,
};

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::mpsc;

impl ThreadHandle {
    pub fn is_closed(&self) -> bool {
        self.inner.ops.is_closed()
    }
    pub async fn submit(&self, op: ThreadOp) -> Result<()> {
        anyhow::ensure!(!self.is_closed(), "agent thread has stopped");
        // Urgent controls must not wait behind a full presentation-event queue.
        // Their managers atomically validate scope, policy revision, and response identity.
        let op = match op {
            ThreadOp::CancelTurn { reply } => {
                if let Some((id, cancel)) = self
                    .inner
                    .cancel
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                {
                    cancel.cancel();
                    self.inner.approvals.cancel_run(id);
                }
                let _ = reply.send(());
                return Ok(());
            }
            ThreadOp::RespondApproval {
                id,
                response,
                reply,
            } => {
                let session = self.session().await.session_id;
                let result = if self
                    .inner
                    .approvals
                    .get(&id)
                    .is_some_and(|record| record.session_id == session)
                {
                    self.inner.approvals.decide(&id, &response)
                } else {
                    Err(anyhow::anyhow!("approval does not belong to this thread"))
                };
                let _ = reply.send(result);
                return Ok(());
            }
            ThreadOp::RespondQuestion {
                session_id,
                id,
                response,
                reply,
            } => {
                let result = if session_id == self.session().await.session_id {
                    self.inner.questions.respond(&session_id, &id, &response)
                } else {
                    Err(anyhow::anyhow!("question does not belong to this thread"))
                };
                let _ = reply.send(result);
                return Ok(());
            }
            op => op,
        };
        self.inner
            .ops
            .send(op)
            .await
            .context("agent thread has stopped")
    }
    pub async fn submit_turn(&self, turn: UserTurn) -> Result<String> {
        self.submit_turn_op(uuid::Uuid::new_v4().to_string(), turn, None)
            .await
    }
    pub async fn submit_hosted_turn(&self, context: HostedTurn, turn: UserTurn) -> Result<String> {
        self.submit_turn_op(context.info.run_id.clone(), turn, Some(Box::new(context)))
            .await
    }
    async fn submit_turn_op(
        &self,
        op_id: String,
        turn: UserTurn,
        context: Option<Box<HostedTurn>>,
    ) -> Result<String> {
        let started = crate::diagnostics::enabled().then(std::time::Instant::now);
        let trace_id = started.map(|_| op_id.clone());
        let (reply, receive) = tokio::sync::oneshot::channel();
        self.submit(ThreadOp::SubmitTurn {
            op_id,
            turn,
            context,
            reply,
        })
        .await?;
        let enqueued_ms = started.map(|started| started.elapsed().as_secs_f64() * 1000.0);
        let result = receive
            .await
            .context("agent thread stopped before accepting the turn")?;
        if let Some(started) = started {
            tracing::info!(target: "kyler_latency", op_id=?trace_id,
                accepted=result.is_ok(), enqueue_ms=?enqueued_ms, elapsed_ms=started.elapsed().as_secs_f64()*1000.0, "turn submission latency");
        }
        result
    }
    pub async fn cancel(&self) -> Result<()> {
        let (reply, receive) = tokio::sync::oneshot::channel();
        self.submit(ThreadOp::CancelTurn { reply }).await?;
        receive
            .await
            .context("agent thread stopped before cancellation")
    }
    pub async fn respond_approval(
        &self,
        id: String,
        response: kyler_protocol::api::permissions::ApprovalResponse,
    ) -> Result<kyler_protocol::api::permissions::ApprovalRecord> {
        let (reply, receive) = tokio::sync::oneshot::channel();
        self.submit(ThreadOp::RespondApproval {
            id,
            response,
            reply,
        })
        .await?;
        receive
            .await
            .context("agent thread stopped before approval response")?
    }
    pub async fn respond_question(
        &self,
        session_id: String,
        id: String,
        response: kyler_protocol::api::questions::QuestionResponse,
    ) -> Result<kyler_protocol::api::questions::QuestionRecord> {
        let (reply, receive) = tokio::sync::oneshot::channel();
        self.submit(ThreadOp::RespondQuestion {
            session_id,
            id,
            response,
            reply,
        })
        .await?;
        receive
            .await
            .context("agent thread stopped before question response")?
    }
    pub async fn take_events(&self) -> Result<mpsc::Receiver<ThreadEvent>> {
        self.inner
            .events
            .lock()
            .await
            .take()
            .context("thread already has an event consumer")
    }
    pub async fn session(&self) -> Session {
        self.inner.session.read().await.clone()
    }
    pub async fn shutdown(&self) -> Result<()> {
        // A claimed event receiver must be drained/dropped by its owner before joining.
        self.inner.events.lock().await.take();
        if !self.is_closed() {
            self.cancel().await?;
            let _ = self.submit(ThreadOp::Shutdown).await;
        }
        if let Some(task) = self.inner.task.lock().await.take() {
            task.await??;
        }
        Ok(())
    }
}

impl ThreadHandle {
    pub async fn history(
        &self,
        cursor: usize,
        limit: usize,
    ) -> Result<kyler_protocol::api::history::HistoryPage> {
        projection::history(&self.session().await, cursor, limit)
    }
    pub async fn tool_details(
        &self,
        call: &str,
    ) -> Result<kyler_protocol::api::tools::ToolDetails> {
        let id = self.session().await.session_id;
        let session = self.inner.store.load(&id).await?;
        projection::tool_details(&session, call)
    }
    pub async fn usage(&self) -> kyler_protocol::api::SessionUsage {
        let session = self.session().await;
        kyler_protocol::api::SessionUsage {
            prompt_tokens: u64::from(session.usage.prompt_tokens)
                + u64::from(session.subagent_usage.prompt_tokens),
            completion_tokens: u64::from(session.usage.completion_tokens)
                + u64::from(session.subagent_usage.completion_tokens),
            total_tokens: u64::from(session.usage.total_tokens)
                + u64::from(session.subagent_usage.total_tokens),
        }
    }
    pub async fn effective_environment(&self) -> Option<Arc<crate::ExecutionEnvironment>> {
        let mut agent = self.inner.agent.clone();
        if let Some(environment) = self.inner.environment.read().await.clone() {
            agent = agent.with_environment(environment);
        }
        let agent = if self.inner.enforce_permissions {
            agent.with_approvals(crate::approval::ApprovalContext {
                manager: self.inner.approvals.clone(),
                policy: self.inner.permissions.read().await.clone(),
                session_id: self.session().await.session_id,
                run_id: String::new(),
            })
        } else {
            agent
        };
        agent.effective_execution_environment()
    }
    pub fn agent(&self) -> &Agent {
        &self.inner.agent
    }
    pub fn approvals(&self) -> &ApprovalManager {
        &self.inner.approvals
    }
    pub fn questions(&self) -> &QuestionManager {
        &self.inner.questions
    }
    pub fn attachments(&self) -> &AttachmentStore {
        &self.inner.attachments
    }
}
