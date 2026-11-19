use crate::{HostEvent, SharedHost, updates};

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use agent_client_protocol::{
    self as acp, Agent, Client, ConnectTo, Error,
    v1::{ConnectionTo, schema::*},
};
use serde_json::json;
use tokio::sync::{Mutex, mpsc, watch};

use kyler_protocol::api::{
    permissions::{ApprovalDecision, ApprovalResponse, ApprovalStatus},
    questions::QuestionStatus,
    run::RunStatus,
};

#[derive(Default)]
struct Control {
    busy: bool,
    cancelled: bool,
}
struct Session {
    _slot: tokio::sync::OwnedSemaphorePermit,
    control: std::sync::Mutex<Control>,
    events: Mutex<mpsc::Receiver<HostEvent>>,
}
struct State {
    host: SharedHost,
    slots: Arc<tokio::sync::Semaphore>,
    initialized: AtomicBool,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}
impl State {
    fn initialized(&self) -> acp::Result<()> {
        if self.initialized.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(Error::invalid_request())
        }
    }
    async fn session(&self, id: &str) -> acp::Result<Arc<Session>> {
        self.initialized()?;
        self.sessions
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(Error::invalid_params)
    }
}
struct Active {
    session: Arc<Session>,
    done: watch::Sender<bool>,
}
impl Drop for Active {
    fn drop(&mut self) {
        let _ = self.done.send(true);
        *self
            .session
            .control
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Control::default();
    }
}

async fn prompt(
    state: Arc<State>,
    request: PromptRequest,
    connection: ConnectionTo<Client>,
) -> acp::Result<PromptResponse> {
    let id = request.session_id.to_string();
    let session = state.session(&id).await?;
    let mut text = String::new();
    for block in request.prompt {
        match block {
            ContentBlock::Text(content) => {
                text.push_str(&content.text);
                text.push('\n');
            }
            ContentBlock::ResourceLink(link) => {
                text.push_str(&format!(
                    "\nReferenced resource (untrusted metadata): {} ({})\n",
                    link.name, link.uri
                ));
            }
            _ => return Err(Error::invalid_params()),
        }
    }
    if text.trim().is_empty() || text.len() > 1024 * 1024 {
        return Err(Error::invalid_params());
    }
    {
        let mut control = session
            .control
            .lock()
            .map_err(|_| Error::internal_error())?;
        if control.busy {
            return Err(Error::invalid_request());
        }
        control.busy = true;
    }
    let (done, _) = watch::channel(false);
    let active = Active {
        session: session.clone(),
        done,
    };
    let result = run_prompt(
        state.clone(),
        id.clone(),
        session,
        text,
        connection,
        &active,
    )
    .await;
    if result.is_err() {
        let _ = state.host.cancel(&id).await;
    }
    result
}

async fn run_prompt(
    state: Arc<State>,
    id: String,
    session: Arc<Session>,
    text: String,
    connection: ConnectionTo<Client>,
    active: &Active,
) -> acp::Result<PromptResponse> {
    let mut events = session.events.lock().await;
    let run = state
        .host
        .submit(&id, text)
        .await
        .map_err(|_| Error::internal_error())?;
    let cancelled = session
        .control
        .lock()
        .map_err(|_| Error::internal_error())?
        .cancelled;
    if cancelled {
        state
            .host
            .cancel(&id)
            .await
            .map_err(|_| Error::internal_error())?;
    }
    let mut emitted = false;
    while let Some(event) = events.recv().await {
        match event {
            HostEvent::Plan { run_id, entries } if run_id == run => {
                let entries=entries.into_iter().map(|entry|json!({"content":entry.content,"priority":"medium","status":match entry.status {crate::PlanStatus::Pending=>"pending",crate::PlanStatus::InProgress=>"in_progress",crate::PlanStatus::Completed=>"completed"}})).collect::<Vec<_>>();
                connection.send_notification(updates::notification(
                    &id,
                    json!({"sessionUpdate":"plan","entries":entries}),
                )?)?;
            }
            HostEvent::Update { run_id, payload } if run_id == run => {
                if let kyler_protocol::api::event::EventPayload::ApprovalUpdated { approval } =
                    &payload
                {
                    if approval.status == ApprovalStatus::Pending && approval.requires_user {
                        let permission = updates::permission(&id, approval)?;
                        let host = state.host.clone();
                        let id = id.clone();
                        let record = approval.clone();
                        let connection = connection.clone();
                        let mut done = active.done.subscribe();
                        let spawn = connection.clone();
                        spawn.spawn(async move {
                            let response = tokio::select! {
                                response=connection.send_request(permission).block_task()=>response,
                                _=done.changed()=>return Ok(()),
                            };
                            let decision = match response {
                                Ok(response) => match response.outcome {
                                    RequestPermissionOutcome::Selected(selected)
                                        if selected.option_id.to_string() == "allow_once" =>
                                    {
                                        ApprovalDecision::AllowOnce
                                    }
                                    _ => ApprovalDecision::Deny,
                                },
                                Err(_) => ApprovalDecision::Deny,
                            };
                            let _ = host
                                .approve(
                                    &id,
                                    record.id,
                                    ApprovalResponse {
                                        decision,
                                        digest: record.digest,
                                    },
                                )
                                .await;
                            Ok(())
                        })?;
                    }
                }
                if let kyler_protocol::api::event::EventPayload::QuestionUpdated { question } =
                    &payload
                {
                    if question.status == QuestionStatus::Pending {
                        connection.send_notification(updates::notification(&id,json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"This ACP connection does not support structured questions; the clarification request was dismissed."}}))?)?;
                        state
                            .host
                            .dismiss_question(&id, question.id.clone())
                            .await
                            .map_err(|_| Error::internal_error())?;
                    }
                }
                if matches!(
                    payload,
                    kyler_protocol::api::event::EventPayload::TextDelta { .. }
                ) {
                    emitted = true;
                }
                if let Some(update) = updates::update(&id, &payload)? {
                    connection.send_notification(update)?;
                }
            }
            HostEvent::Finished(info) if info.run_id == run => {
                if info.status == RunStatus::Completed && !emitted {
                    if let Some(output) = info.output {
                        connection.send_notification(updates::notification(&id,json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":output}}))?)?;
                    }
                }
                return match info.status {
                    RunStatus::Completed => Ok(PromptResponse::new(StopReason::EndTurn)),
                    RunStatus::Cancelled => Ok(PromptResponse::new(StopReason::Cancelled)),
                    _ => Err(Error::internal_error()),
                };
            }
            _ => {}
        }
    }
    Err(Error::internal_error())
}

pub async fn serve_stdio(host: SharedHost) -> anyhow::Result<()> {
    serve(host, acp::Stdio::new()).await
}
/// Serve one connection; all sessions are connection-owned and cleaned up on EOF/error.
pub async fn serve(
    host: SharedHost,
    transport: impl ConnectTo<Agent> + 'static,
) -> anyhow::Result<()> {
    let state = Arc::new(State {
        host: host.clone(),
        slots: Arc::new(tokio::sync::Semaphore::new(128)),
        initialized: AtomicBool::new(false),
        sessions: Mutex::new(HashMap::new()),
    });
    let initialize = state.clone();
    let new = state.clone();
    let prompts = state.clone();
    let cancel = state.clone();
    let result = Agent
        .v1()
        .name("kyler")
        .on_receive_request(
            async move |_request: InitializeRequest, responder, _| {
                if initialize.initialized.swap(true, Ordering::AcqRel) {
                    return responder.respond_with_error(Error::invalid_request());
                }
                responder.respond(
                    InitializeResponse::new(acp::schema::ProtocolVersion::V1)
                        .agent_capabilities(AgentCapabilities::new()),
                )
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest, responder, connection| {
                let state = new.clone();
                connection.spawn(async move {
                    let result = async {
                        state.initialized()?;
                        if !request.cwd.is_absolute() {
                            return Err(Error::invalid_params().data("cwd must be an absolute directory"));
                        }
                        if !request.mcp_servers.is_empty() {
                            return Err(Error::invalid_params().data("Client-supplied MCP servers are not supported; configure connectors in the Kyler host configuration"));
                        }
                        let slot = state
                            .slots
                            .clone()
                            .try_acquire_owned()
                            .map_err(|_| Error::invalid_request())?;
                        let session = state
                            .host
                            .new_session(&request.cwd)
                            .await
                            .map_err(|_| Error::invalid_params())?;
                        let id = session.id;
                        state.sessions.lock().await.insert(
                            id.clone(),
                            Arc::new(Session {
                                _slot: slot,
                                control: Default::default(),
                                events: Mutex::new(session.events),
                            }),
                        );
                        Ok(NewSessionResponse::new(id))
                    }
                    .await;
                    responder.respond_with_result(result)
                })
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PromptRequest, responder, connection| {
                let state = prompts.clone();
                let cloned = connection.clone();
                connection.spawn(async move {
                    responder.respond_with_result(prompt(state, request, cloned).await)
                })
            },
            acp::on_receive_request!(),
        )
        .on_receive_notification(
            async move |request: CancelNotification, connection| {
                let state = cancel.clone();
                connection.spawn(async move {
                    let id = request.session_id.to_string();
                    if let Ok(session) = state.session(&id).await {
                        let active = {
                            let mut control = session
                                .control
                                .lock()
                                .map_err(|_| Error::internal_error())?;
                            if control.busy {
                                control.cancelled = true;
                            }
                            control.busy
                        };
                        if active {
                            let _ = state.host.cancel(&id).await;
                        }
                    }
                    Ok(())
                })
            },
            acp::on_receive_notification!(),
        )
        .connect_to(transport)
        .await;
    state.sessions.lock().await.clear();
    let cleanup = host.shutdown().await;
    result?;
    cleanup
}
