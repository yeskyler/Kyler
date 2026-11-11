//! A single core consumer feeds the service event cache; SSE clients never own execution.
use crate::runs::{
    RunManager,
    events::{Entry, prefix},
    persistence,
};

use std::{sync::Arc, time::Duration};

use kyler_core::{
    Agent, Session,
    runtime::UserTurn,
    thread::{HostedTurn, ThreadEvent},
};
use kyler_protocol::api::{ApiErrorBody, event::EventPayload, run::RunStatus};

fn failure(code: &str, message: &str) -> ApiErrorBody {
    ApiErrorBody {
        code: code.into(),
        message: message.into(),
    }
}
impl RunManager {
    pub(crate) async fn drive(
        &self,
        entry: Arc<Entry>,
        session: Session,
        turn: UserTurn,
        agent: Agent,
    ) -> Result<(), ApiErrorBody> {
        let thread = self
            .core_thread(&session)
            .await
            .map_err(|_| failure("execution_failed", "The agent thread could not start."))?;
        // One bridge consumes this thread at a time; each admitted run has one session owner.
        let mut events = thread.events.lock().await;
        thread
            .handle
            .submit_hosted_turn(
                HostedTurn {
                    agent: agent.clone(),
                    session,
                    info: entry.info(),
                    timeout: Duration::from_secs(self.settings.run_timeout_seconds),
                },
                turn,
            )
            .await
            .map_err(|_| failure("execution_failed", "The agent could not accept the turn."))?;
        let mut requested = entry.cancel.subscribe();
        let mut cancelled = false;
        let mut storage_error = None;
        let mut response_published = false;
        let final_info = loop {
            tokio::select! {
                biased;
                _ = async {let _ = requested.wait_for(|v|*v).await;}, if !cancelled => {
                    cancelled = true;
                    thread.handle.cancel().await.map_err(|_|failure("execution_failed","The agent thread could not be cancelled."))?;
                },
                event = events.recv() => match event {
                    Some(ThreadEvent::Event(event)) => {
                        if event.op_id != entry.info().run_id {continue;}
                        if matches!(event.payload,EventPayload::RunStarted {..}) {
                            {let mut data = entry.inner.lock().unwrap_or_else(|e|e.into_inner());data.info.execution_id = event.execution_id;if data.info.status!=RunStatus::Cancelling {data.info.status=RunStatus::Running;}}
                            if persistence::save(self.root.clone(),entry.info()).await.is_err() {
                                storage_error = Some(failure("storage_error","The run could not be persisted."));
                                thread.handle.cancel().await.ok();
                            }
                        }
                        let visible = matches!(&event.payload,EventPayload::TextDelta {text,..} if !text.trim().is_empty())
                            || matches!(&event.payload,EventPayload::QuestionUpdated {question} if question.status==kyler_protocol::api::questions::QuestionStatus::Pending);
                        if visible && !response_published {
                            if self.publish_session(&entry.info().session_id).await.is_err() {
                                storage_error=Some(failure("storage_error","The conversation could not be saved."));
                                thread.handle.cancel().await.ok();continue;
                            }
                            response_published=true;
                        }
                        self.publish_core_event(&entry,event.payload);
                    },
                    Some(ThreadEvent::TurnFinished(info)) if info.run_id==entry.info().run_id => break info,
                    Some(ThreadEvent::TurnFinished(_)) => {},
                    Some(ThreadEvent::ShutdownComplete) | None => return Err(failure("execution_failed","The agent thread stopped before its terminal event.")),
                }
            }
        };
        if final_info.status == RunStatus::Completed && storage_error.is_none() {
            let snapshot = thread.handle.session().await;
            if let Some(execution) = snapshot.execution {
                let artifacts = self.artifacts.clone();
                let environment = thread.handle.effective_environment().await;
                let output = final_info.output.clone().unwrap_or_default();
                if !matches!(
                    tokio::task::spawn_blocking(move || artifacts.capture_registered(
                        &snapshot.session_id,
                        &execution.execution_id,
                        &output,
                        environment.as_deref(),
                        &execution.deliverables
                    ))
                    .await,
                    Ok(Ok(()))
                ) {
                    tracing::warn!("Some final-response artifacts could not be retained");
                }
            }
        }
        self.finish(&entry, Some(final_info), storage_error).await;
        Ok(())
    }
    fn publish_core_event(&self, entry: &Entry, payload: EventPayload) {
        let mut payload = payload;
        match &mut payload {
            EventPayload::ToolCallDelta { call_id, name }
            | EventPayload::ToolQueued { call_id, name, .. }
            | EventPayload::ToolStarted { call_id, name, .. }
            | EventPayload::ToolFinished { call_id, name, .. } => {
                *call_id = prefix(call_id, 256);
                *name = prefix(name, 256);
            }
            _ => {}
        }
        let reasoning = matches!(payload, EventPayload::ReasoningDelta { .. });
        match payload {
            EventPayload::TextDelta { step, text }
            | EventPayload::ReasoningDelta { step, text } => {
                // Network retention and chunk limits remain independent of core backpressure.
                let cap = (self.settings.event_buffer_max_bytes / 8).min(16384);
                let mut rest = text.as_str();
                while !rest.is_empty() {
                    let part = prefix(rest, cap);
                    rest = &rest[part.len()..];
                    entry.publish(if reasoning {
                        EventPayload::ReasoningDelta { step, text: part }
                    } else {
                        EventPayload::TextDelta { step, text: part }
                    });
                }
            }
            payload => entry.publish(payload),
        }
    }
}
