use super::{ThreadEvent, ThreadOp, ThreadState};
use crate::{AgentStream, AgentStreamEvent, approval::ApprovalContext, context::ToolResultStatus};

use std::{collections::HashMap, sync::Arc, time::Duration};

use sha2::{Digest, Sha256};

use anyhow::Result;
use tokio::sync::mpsc;

use kyler_protocol::api::{
    ApiErrorBody,
    event::EventPayload,
    run::{RunInfo, RunStatus, Usage},
};

pub(super) async fn run(
    state: Arc<ThreadState>,
    mut ops: mpsc::Receiver<ThreadOp>,
    events: mpsc::Sender<ThreadEvent>,
) -> Result<()> {
    let mut stream: Option<AgentStream> = None;
    let mut approvals = state.approvals.subscribe();
    let mut questions = state.questions.subscribe();
    let mut stopping = false;
    let mut seq = 0;
    let mut receipts: HashMap<String, Vec<u8>> = HashMap::new();
    let mut deadline = None;
    let mut elapsed = Duration::ZERO;
    let mut previous = tokio::time::Instant::now();
    let mut timed_out = false;
    let mut clock = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            biased;
            op = ops.recv(), if !stopping => match op {
                Some(ThreadOp::SubmitTurn {op_id,turn,context,reply}) => {
                    let fingerprint = Sha256::digest(serde_json::to_vec(&(&turn.text,&turn.images,&turn.attachments,turn.resume))?).to_vec();
                    if let Some(previous) = receipts.get(&op_id) {
                        let result = if previous == &fingerprint {Ok(op_id)} else {Err(anyhow::anyhow!("operation ID was already used with different input"))};
                        let _ = reply.send(result); continue;
                    }
                    if stream.is_some() {let _ = reply.send(Err(anyhow::anyhow!("thread is already running a turn")));continue;}
                    let timeout = context.as_ref().map(|c|c.timeout);
                    match start_turn(&state,op_id.clone(),turn,context).await {
                        Ok(next) => {
                            stream = Some(next);seq = 0;deadline = timeout;elapsed = Duration::ZERO;previous = tokio::time::Instant::now();timed_out = false;
                            receipts.insert(op_id.clone(),fingerprint);
                            if let Some(policy) = state.active.read().await.as_ref().and_then(|i|i.permissions.clone()) {
                                publish(&state,&events,&mut seq,EventPayload::PermissionsBound {policy}).await;
                            }
                            let _ = reply.send(Ok(op_id));
                        },
                        Err(error) => {let _ = reply.send(Err(error));},
                    }
                },
                Some(ThreadOp::SelectModel {profile,reply}) => {
                    let handle = super::ThreadHandle {inner:state.clone()};
                    let _ = reply.send(handle.model_state(Some(profile)).await);
                },
                Some(ThreadOp::SelectThinking {level,reply}) => {
                    let handle = super::ThreadHandle {inner:state.clone()};
                    let _ = reply.send(handle.thinking_state(Some(level)).await);
                },
                Some(ThreadOp::SelectPermissions {selection,reply}) => {
                    let handle = super::ThreadHandle {inner:state.clone()};
                    let result = handle.permission_state(Some(&selection)).await;
                    if let Ok(selected) = &result {publish(&state,&events,&mut seq,EventPayload::PermissionsBound {policy:selected.selected.clone()}).await;}
                    let _ = reply.send(result);
                },
                Some(ThreadOp::RespondApproval {id,response,reply}) => {let _ = reply.send(state.approvals.decide(&id,&response));},
                Some(ThreadOp::RespondQuestion {session_id,id,response,reply}) => {let _ = reply.send(state.questions.respond(&session_id,&id,&response));},
                Some(ThreadOp::CancelTurn {reply}) => {
                    if let Some(stream) = &stream {stream.cancel();}
                    if let Some(active) = state.active.read().await.as_ref() {state.approvals.cancel_run(&active.run_id);}
                    let _ = reply.send(());
                },
                Some(ThreadOp::Shutdown) | None => {
                    stopping = true;
                    if let Some(stream) = &stream {stream.cancel();}
                },
            },
            _ = events.closed(), if !stopping => {stopping = true; if let Some(stream) = &stream {stream.cancel();}},
            _ = clock.tick(), if stream.is_some() && deadline.is_some() && !timed_out => {
                let now = tokio::time::Instant::now();
                let id = state.session.read().await.session_id.clone();
                let run_id = state.active.read().await.as_ref().map(|i|i.run_id.clone()).unwrap_or_default();
                if !state.questions.waiting(&id) && !state.approvals.waiting(&run_id) {elapsed += now - previous;}
                previous = now;
                if elapsed >= deadline.unwrap() {timed_out = true;stream.as_ref().unwrap().cancel();state.approvals.cancel_run(&run_id);}
            },
            update = approvals.recv(), if stream.is_some() => {
                match update {Ok(approval) => {if state.active.read().await.as_ref().is_some_and(|i|i.run_id==approval.run_id) {publish(&state, &events, &mut seq, EventPayload::ApprovalUpdated {approval}).await;}},
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        for approval in state.approvals.list(&state.session.read().await.session_id) { publish(&state, &events, &mut seq, EventPayload::ApprovalUpdated {approval}).await; }
                    }, _ => {}}
            },
            update = questions.recv(), if stream.is_some() => {
                match update {Ok(question) => {if question.session_id==state.session.read().await.session_id {publish(&state, &events, &mut seq, EventPayload::QuestionUpdated {question}).await;}},
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        for question in state.questions.list(&state.session.read().await.session_id) {publish(&state, &events, &mut seq, EventPayload::QuestionUpdated {question}).await;}
                    }, _ => {}}
            },
            event = async { stream.as_mut().unwrap().next().await }, if stream.is_some() => {
                if let Some(event) = event {
                    if let AgentStreamEvent::Started {execution_id, ..} = &event {
                        if let Some(active) = state.active.write().await.as_mut() {active.execution_id = Some(execution_id.clone());}
                    }
                    if let Some(payload) = projection(event, &state).await {publish(&state, &events, &mut seq, payload).await;}
                } else {
                    finish_turn(&state, stream.take().unwrap(), &events, &mut seq,timed_out).await?;
                }
            },
        }
        if stopping && stream.is_none() {
            break;
        }
    }
    Ok(())
}
async fn start_turn(
    state: &ThreadState,
    id: String,
    turn: crate::runtime::UserTurn,
    context: Option<Box<super::HostedTurn>>,
) -> Result<AgentStream> {
    anyhow::ensure!(
        !id.is_empty() && id.len() <= 128,
        "operation ID must contain 1 to 128 bytes"
    );
    let (session, agent, host_info) = match context {
        Some(context) => {
            anyhow::ensure!(state.hosted, "only hosted threads accept a host context");
            anyhow::ensure!(
                context.session.session_id == state.session.read().await.session_id
                    && context.info.run_id == id
                    && context.info.session_id == context.session.session_id,
                "hosted turn identity mismatch"
            );
            *state.session.write().await = context.session.clone();
            (context.session, context.agent, Some(context.info))
        }
        None => {
            let session = state.session.read().await.clone();
            let profile = state.profile.read().await.clone();
            (session, state.agent.bind_model(profile.as_deref())?, None)
        }
    };
    let thinking_path = if state.hosted {
        state
            .preferences
            .join(format!("{}.thinking", session.session_id))
    } else {
        state.preferences.with_extension("thinking")
    };
    let agent = if agent.thinking_supported() {
        match super::preferences::read_thinking(&thinking_path)? {
            Some(level) => {
                let level = if !agent.thinking_levels().contains(&level) {
                    kyler_protocol::api::models::ThinkingLevel::High
                } else {
                    level
                };
                agent.with_thinking_level(level)?
            }
            None => agent.with_thinking_level(kyler_protocol::api::models::ThinkingLevel::High)?,
        }
    } else {
        agent
    };
    *state.environment.write().await = agent.execution_environment();
    let permissions = state.permissions.read().await.clone();
    if turn.resume {
        anyhow::ensure!(
            session.execution.as_ref().is_some_and(|e| matches!(
                e.status,
                crate::ExecutionStatus::Failed | crate::ExecutionStatus::Cancelled
            )),
            "Only failed or cancelled tasks can be resumed"
        );
    } else {
        anyhow::ensure!(
            !turn.text.trim().is_empty() || !turn.attachments.is_empty() || !turn.images.is_empty(),
            "Enter a message or attach a file"
        );
    }
    for attachment in &turn.attachments {
        if attachment.kind == kyler_protocol::api::attachments::AttachmentKind::Image {
            agent.validate_image_input()?;
        } else {
            agent.validate_document_input()?;
        }
    }
    if !turn.images.is_empty() {
        agent.validate_image_input()?;
    }
    let model = agent.model_info();
    let agent = if state.enforce_permissions {
        state
            .approvals
            .update_policy(&session.session_id, permissions.clone());
        agent.with_approvals(ApprovalContext {
            manager: state.approvals.clone(),
            policy: permissions.clone(),
            session_id: session.session_id.clone(),
            run_id: id.clone(),
        })
    } else {
        agent
    };
    let stream = agent.run_turn_in_session(session.clone(), turn).await?;
    let mut info = host_info.unwrap_or(RunInfo {
        run_id: id.clone(),
        session_id: session.session_id,
        execution_id: None,
        status: RunStatus::Running,
        created_at: chrono::Utc::now().timestamp_millis(),
        finished_at: None,
        last_event_seq: 0,
        output: None,
        usage: None,
        error: None,
        model,
        permissions: state.enforce_permissions.then_some(permissions.clone()),
    });
    if state.enforce_permissions {
        info.permissions = Some(permissions);
    }
    *state.active.write().await = Some(info);
    *state.cancel.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((id.clone(), stream.cancel_handle()));
    Ok(stream)
}
async fn finish_turn(
    state: &ThreadState,
    stream: AgentStream,
    events: &mpsc::Sender<ThreadEvent>,
    seq: &mut u64,
    timed_out: bool,
) -> Result<()> {
    let outcome = stream.finish_with_session().await?;
    state
        .cancel
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    let mut info = state.active.write().await.take().unwrap();
    state.approvals.cancel_run(&info.run_id);
    info.finished_at = Some(chrono::Utc::now().timestamp_millis());
    let previous_id = state
        .session
        .read()
        .await
        .execution
        .as_ref()
        .map(|e| e.execution_id.clone());
    let execution = outcome.session.execution.as_ref().filter(|e| {
        info.execution_id.as_deref() == Some(e.execution_id.as_str())
            || previous_id.as_deref() != Some(e.execution_id.as_str())
    });
    info.execution_id = execution.map(|e| e.execution_id.clone());
    info.usage = execution.map(|e| Usage {
        prompt_tokens: e.usage.prompt_tokens,
        completion_tokens: e.usage.completion_tokens,
        total_tokens: e.usage.total_tokens,
    });
    match outcome.result {
        Ok(result) => {
            info.status = RunStatus::Completed;
            info.output = Some(result.output);
        }
        Err(_) if execution.is_some_and(|e| e.status == crate::ExecutionStatus::Cancelled) => {
            info.status = RunStatus::Cancelled;
        }
        Err(error) => {
            info.status = RunStatus::Failed;
            info.error = Some(
                error
                    .downcast_ref::<crate::models::error::ModelError>()
                    .map(|e| e.public_error())
                    .unwrap_or_else(|| ApiErrorBody {
                        code: "execution_failed".into(),
                        message: "Execution failed; inspect the local session rollout.".into(),
                    }),
            );
        }
    }
    if timed_out {
        info.output = None;
        info.status = RunStatus::Failed;
        info.error = Some(ApiErrorBody {
            code: "run_timeout".into(),
            message: "The execution exceeded its time limit.".into(),
        });
    }
    *state.session.write().await = outcome.session;
    for question in state
        .questions
        .list(&info.session_id)
        .into_iter()
        .filter(|q| Some(q.execution_id.as_str()) == info.execution_id.as_deref())
    {
        publish_info(
            &state.event_timing,
            events,
            seq,
            &info,
            EventPayload::QuestionUpdated { question },
        )
        .await;
    }
    for approval in state
        .approvals
        .list(&info.session_id)
        .into_iter()
        .filter(|a| a.run_id == info.run_id)
    {
        publish_info(
            &state.event_timing,
            events,
            seq,
            &info,
            EventPayload::ApprovalUpdated { approval },
        )
        .await;
    }
    info.last_event_seq = *seq;
    let _ = events.send(ThreadEvent::TurnFinished(info)).await;
    Ok(())
}

async fn publish(
    state: &ThreadState,
    events: &mpsc::Sender<ThreadEvent>,
    seq: &mut u64,
    payload: EventPayload,
) {
    if let Some(info) = state.active.read().await.as_ref() {
        publish_info(&state.event_timing, events, seq, info, payload).await;
    }
}
async fn publish_info(
    timing: &crate::diagnostics::ChannelTiming,
    events: &mpsc::Sender<ThreadEvent>,
    seq: &mut u64,
    info: &RunInfo,
    payload: EventPayload,
) {
    *seq += 1;
    let _ = timing
        .send(
            events,
            ThreadEvent::Event(super::ThreadNotification {
                thread_id: info.session_id.clone(),
                op_id: info.run_id.clone(),
                execution_id: info.execution_id.clone(),
                seq: *seq,
                timestamp_ms: chrono::Utc::now().timestamp_millis(),
                payload,
            }),
        )
        .await;
}
async fn projection(event: AgentStreamEvent, state: &ThreadState) -> Option<EventPayload> {
    Some(match event {
        AgentStreamEvent::Started { session_id, .. } => EventPayload::RunStarted {
            session_id,
            model: state
                .active
                .read()
                .await
                .as_ref()
                .and_then(|a| a.model.clone()),
        },
        AgentStreamEvent::ContextCompaction { compaction } => {
            EventPayload::ContextCompaction { compaction }
        }
        AgentStreamEvent::StepStarted { step } => EventPayload::StepStarted { step },
        AgentStreamEvent::ReasoningDelta { step, text } => {
            EventPayload::ReasoningDelta { step, text }
        }
        AgentStreamEvent::TextDelta { step, text } => EventPayload::TextDelta { step, text },
        AgentStreamEvent::ToolCallDelta { call_id, name } => {
            EventPayload::ToolCallDelta { call_id, name }
        }
        AgentStreamEvent::ToolQueued {
            call_id,
            name,
            arguments,
            activity,
            source,
        } => EventPayload::ToolQueued {
            call_id,
            summary: super::projection::tool_summary(&name, &arguments, &source),
            name,
            activity,
            source,
        },
        AgentStreamEvent::ToolStarted {
            call_id,
            name,
            arguments,
            activity,
            source,
        } => EventPayload::ToolStarted {
            call_id,
            summary: super::projection::tool_summary(&name, &arguments, &source),
            name,
            activity,
            source,
        },
        AgentStreamEvent::ToolFinished {
            call_id,
            name,
            status,
            output,
        } => EventPayload::ToolFinished {
            call_id,
            name,
            status: if status == ToolResultStatus::Success {
                "success"
            } else {
                "error"
            }
            .into(),
            output_bytes: output.len(),
            error: if status == ToolResultStatus::Success {
                None
            } else {
                output
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .map(|line| super::projection::prefix(line, 240))
            },
        },
        AgentStreamEvent::Completed { .. }
        | AgentStreamEvent::Failed { .. }
        | AgentStreamEvent::Cancelled => return None,
    })
}
