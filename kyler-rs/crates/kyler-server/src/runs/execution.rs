use super::{
    events::{Entry, prefix},
    manager::{RunManager, public_error, usage},
    persistence,
};
use crate::error::ApiResult;

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::time::Instant;

use kyler_core::{
    Agent, AgentStreamEvent, AgentStreamOutcome, ExecutionStatus, Session,
    models::error::ModelError, runtime::UserTurn,
};
use kyler_protocol::api::{
    ApiErrorBody,
    event::EventPayload,
    run::{RunInfo, RunStatus},
};

impl RunManager {
    pub(super) async fn drive(
        &self,
        entry: Arc<Entry>,
        session: Session,
        turn: UserTurn,
        agent: Agent,
    ) -> Result<(), ApiErrorBody> {
        let mut question_events = self.questions.subscribe();
        let mut question_states = HashMap::new();
        let mut approval_events = self.approvals.as_ref().map(|m| m.subscribe());
        if let Some(policy) = entry.info().permissions {
            entry.publish(EventPayload::PermissionsBound { policy });
        }
        let mut stream = agent
            .run_turn_in_session(session, turn)
            .await
            .map_err(|_| public_error("execution_failed", "The agent could not start."))?;
        let cancel = stream.cancel_handle();
        let mut requested = entry.cancel.subscribe();
        let mut clock = tokio::time::interval(Duration::from_millis(100));
        let mut previous = Instant::now();
        let mut elapsed = Duration::ZERO;
        let mut timed_out = false;
        let mut cancelled = false;
        let mut storage_error = None;
        let mut response_published = false;
        loop {
            tokio::select! {
                biased;
                _ = async {let _=requested.wait_for(|v|*v).await;}, if !cancelled => {cancelled=true;cancel.cancel();}
                _ = clock.tick(), if !cancelled => {
                    let now = tokio::time::Instant::now();
                    if !self.questions.waiting(&entry.info().session_id) && self.approvals.as_ref().is_none_or(|m| !m.waiting(&entry.info().run_id)) { elapsed += now - previous; }
                    previous = now;
                    if elapsed >= Duration::from_secs(self.settings.run_timeout_seconds) { timed_out=true;cancelled=true;cancel.cancel();entry.inner.lock().unwrap_or_else(|e|e.into_inner()).info.status=RunStatus::Cancelling; }
                }
                update = question_events.recv() => {
                    if let Ok(question) = update {
                        if question.session_id == entry.info().session_id {
                            if !response_published && question.status == kyler_protocol::api::questions::QuestionStatus::Pending {
                                if self.publish_session(&entry.info().session_id).await.is_err() {
                                    storage_error=Some(public_error("storage_error","The conversation could not be saved."));
                                    cancelled=true;cancel.cancel();continue;
                                }
                                response_published=true;
                            }
                            question_states.insert(question.id.clone(), question.status);
                            entry.publish(EventPayload::QuestionUpdated { question });
                        }
                    }
                }
                update = async { match &mut approval_events { Some(events) => events.recv().await, None => std::future::pending().await } } => {
                    if let Ok(approval) = update { if approval.run_id == entry.info().run_id { entry.publish(EventPayload::ApprovalUpdated { approval }); } }
                }
                event=stream.next()=> {
                    let Some(event)=event else {break;};
                    match event {
                        AgentStreamEvent::Started {execution_id,..}=> {
                            {let mut data=entry.inner.lock().unwrap_or_else(|e|e.into_inner());data.info.execution_id=Some(execution_id);if data.info.status!=RunStatus::Cancelling {data.info.status=RunStatus::Running;}}
                            if persistence::save(self.root.clone(),entry.info()).await.is_err() {
                                storage_error=Some(public_error("storage_error","The run could not be persisted."));
                                cancelled=true;cancel.cancel();
                            }
                            entry.publish(EventPayload::RunStarted {session_id:entry.info().session_id, model:entry.info().model});
                        }
                        AgentStreamEvent::StepStarted {step}=>entry.publish(EventPayload::StepStarted {step}),
                        AgentStreamEvent::ReasoningDelta {step,text}=> {
                            let cap=(self.settings.event_buffer_max_bytes/8).min(16384);let mut rest=text.as_str();
                            while !rest.is_empty() {let part=prefix(rest,cap);rest=&rest[part.len()..];entry.publish(EventPayload::ReasoningDelta {step,text:part});}
                        }
                        AgentStreamEvent::TextDelta {step,text}=> {
                            if !response_published && !text.trim().is_empty() {
                                if self.publish_session(&entry.info().session_id).await.is_err() {
                                    storage_error=Some(public_error("storage_error","The conversation could not be saved."));
                                    cancelled=true;cancel.cancel();continue;
                                }
                                response_published=true;
                            }
                            let cap=(self.settings.event_buffer_max_bytes/8).min(16384);let mut rest=text.as_str();
                            while !rest.is_empty() {let part=prefix(rest,cap);rest=&rest[part.len()..];entry.publish(EventPayload::TextDelta {step,text:part});}
                        }
                        AgentStreamEvent::ToolQueued {call_id,name,arguments,activity,source}=>entry.publish(EventPayload::ToolQueued {call_id:prefix(&call_id,256),summary:tool_summary(&name,&arguments,&source),name:prefix(&name,256),activity,source}),
                        AgentStreamEvent::ToolStarted {call_id,name,arguments,activity,source}=>entry.publish(EventPayload::ToolStarted {call_id:prefix(&call_id,256),summary:tool_summary(&name,&arguments,&source),name:prefix(&name,256),activity,source}),
                        AgentStreamEvent::ToolFinished {call_id,name,status,output}=>entry.publish(EventPayload::ToolFinished {call_id:prefix(&call_id,256),name:prefix(&name,256),status:if status==kyler_core::context::ToolResultStatus::Success {"success"} else {"error"}.into(),output_bytes:output.len(),error:if status==kyler_core::context::ToolResultStatus::Success {None} else {output.lines().find(|line|!line.trim().is_empty()).map(|line|prefix(line,240))}}),
                        // Only publish a terminal event after joining and persisting final state.
                        AgentStreamEvent::Completed {..}|AgentStreamEvent::Failed {..}|AgentStreamEvent::Cancelled=>{}
                    }
                }
            }
        }
        let outcome = stream.finish_with_session().await.map_err(|_| {
            public_error(
                "execution_failed",
                "The execution task ended unexpectedly; tool outcomes may be unknown.",
            )
        })?;
        // Resolve a question before the terminal event even if cancellation ended the stream
        // before its broadcast update was consumed.
        for question in self.questions.list(&entry.info().session_id) {
            if entry.info().execution_id.as_deref() == Some(question.execution_id.as_str())
                && question_states.get(&question.id) != Some(&question.status)
                && question.status != kyler_protocol::api::questions::QuestionStatus::Pending
            {
                entry.publish(EventPayload::QuestionUpdated { question });
            }
        }
        let execution_id = entry.info().execution_id;
        let was_cancelled = outcome.session.execution.as_ref().is_some_and(|e| {
            Some(&e.execution_id) == execution_id.as_ref() && e.status == ExecutionStatus::Cancelled
        });
        let error = if storage_error.is_some() {
            storage_error
        } else if timed_out {
            Some(public_error(
                "run_timeout",
                "The execution exceeded its time limit.",
            ))
        } else if let Err(error) = &outcome.result {
            if was_cancelled {
                None
            } else {
                Some(
                    error
                        .downcast_ref::<ModelError>()
                        .map(|error| error.public_error())
                        .unwrap_or_else(|| {
                            public_error(
                                "execution_failed",
                                "The execution failed. Inspect the local rollout for details.",
                            )
                        }),
                )
            }
        } else {
            None
        };
        if error.is_none() {
            if let Ok(result) = &outcome.result {
                let artifacts = self.artifacts.clone();
                let session = outcome.session.session_id.clone();
                let execution = result.context.execution_id.clone();
                let output = result.output.clone();
                let deliverables = result.context.deliverables.clone();
                let environment = agent.effective_execution_environment();
                if !matches!(
                    tokio::task::spawn_blocking(move || artifacts.capture_registered(
                        &session,
                        &execution,
                        &output,
                        environment.as_deref(),
                        &deliverables
                    ))
                    .await,
                    Ok(Ok(()))
                ) {
                    tracing::warn!("Some final-response artifacts could not be retained");
                }
            }
        }
        self.finish(&entry, Some(outcome), error).await;
        Ok(())
    }
    pub(super) async fn finish(
        &self,
        entry: &Entry,
        outcome: Option<AgentStreamOutcome>,
        error: Option<ApiErrorBody>,
    ) {
        let mut info = entry.info();
        info.finished_at = Some(chrono::Utc::now().timestamp_millis());
        if let Some(error) = error {
            info.status = RunStatus::Failed;
            info.error = Some(error);
        } else if let Some(outcome) = outcome {
            match outcome.result {
                Ok(result) => {
                    info.status = RunStatus::Completed;
                    info.output = Some(result.output);
                    info.usage = Some(usage(result.context.usage));
                }
                Err(_) => {
                    info.status = RunStatus::Cancelled;
                    info.usage = outcome.session.execution.map(|e| usage(e.usage));
                }
            }
        } else {
            info.status = RunStatus::Failed;
            info.error = Some(public_error("execution_failed", "The execution failed."));
        }
        if info
            .output
            .as_ref()
            .is_some_and(|text| !text.trim().is_empty())
            && self.publish_session(&info.session_id).await.is_err()
        {
            info.status = RunStatus::Failed;
            info.error = Some(public_error(
                "storage_error",
                "The conversation could not be saved.",
            ));
        }
        info.last_event_seq += 1;
        if persistence::save(self.root.clone(), info.clone())
            .await
            .is_err()
        {
            info.status = RunStatus::Failed;
            info.output = None;
            info.error = Some(public_error(
                "storage_error",
                "The final run record could not be persisted.",
            ));
            tracing::error!(run_id=%info.run_id,"final run manifest write failed");
        }
        let payload = match info.status {
            RunStatus::Completed => {
                let output = info.output.as_deref().unwrap_or_default();
                let preview = prefix(output, self.settings.event_buffer_max_bytes / 8);
                EventPayload::RunCompleted {
                    output_truncated: preview.len() != output.len(),
                    output: preview,
                    usage: info.usage.clone().unwrap_or_default(),
                }
            }
            RunStatus::Cancelled => EventPayload::RunCancelled,
            _ => EventPayload::RunFailed {
                error: info
                    .error
                    .clone()
                    .unwrap_or_else(|| public_error("execution_failed", "The execution failed.")),
            },
        };
        entry.complete(info, payload);
    }
    pub async fn cancel(&self, id: &str) -> ApiResult<RunInfo> {
        let entry = self.entry(id).await?;
        {
            let mut data = entry.inner.lock().unwrap_or_else(|e| e.into_inner());
            if !data.info.status.is_terminal() {
                data.info.status = RunStatus::Cancelling;
                if let Some(approvals) = &self.approvals {
                    approvals.cancel_run(id);
                }
                entry.cancel.send_replace(true);
            }
        }
        Ok(entry.info())
    }
}

/// Only known display fields are included; arbitrary connector arguments and command
/// arguments may contain secrets and remain behind the existing tool-detail endpoint.
pub(crate) fn tool_summary(
    name: &str,
    arguments: &serde_json::Value,
    source: &kyler_protocol::activity::ToolSource,
) -> Option<String> {
    if !matches!(source, kyler_protocol::activity::ToolSource::Builtin) {
        return None;
    }
    let value = match name {
        "read_file" => arguments["file_path"].as_str(),
        "list_dir" => arguments["dir_path"].as_str(),
        "grep_files" => arguments["path"].as_str(),
        "run_command" => arguments["program"].as_str(),
        _ => None,
    }?;
    Some(prefix(
        &value.split_whitespace().collect::<Vec<_>>().join(" "),
        160,
    ))
}

#[cfg(test)]
mod presentation_tests {
    use super::tool_summary;

    #[test]
    fn display_summaries_are_bounded_and_do_not_copy_arbitrary_arguments() {
        use kyler_protocol::activity::ToolSource;

        let args = serde_json::json!({"program":"cargo","args":["secret"],"api_key":"secret"});
        assert_eq!(
            tool_summary("run_command", &args, &ToolSource::Builtin).as_deref(),
            Some("cargo")
        );
        assert!(tool_summary("remote", &args, &ToolSource::Builtin).is_none());
        let source = ToolSource::Mcp {
            server_id: "remote".into(),
            remote_name: "read_file".into(),
        };
        assert!(
            tool_summary(
                "read_file",
                &serde_json::json!({"file_path":"secret"}),
                &source
            )
            .is_none()
        );
        let summary = tool_summary(
            "read_file",
            &serde_json::json!({"file_path":"a".repeat(500)}),
            &ToolSource::Builtin,
        )
        .unwrap();
        assert!(summary.len() <= 160);
    }
}
