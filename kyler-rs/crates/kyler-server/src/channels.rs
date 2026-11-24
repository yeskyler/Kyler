use crate::{
    error::{ApiError, ApiResult},
    runs::RunManager,
    state::AppState,
};

use std::{path::Path as FsPath, sync::Arc, time::Duration};

use anyhow::Result;
use axum::{
    Extension, Json,
    body::Bytes,
    extract::{OriginalUri, Path, Query, RawQuery, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use kyler_channel::{Adapter, Binding, CHANNELS, ChannelStore, Job, Settings, Webhook};
use kyler_core::Session;
use kyler_gateway::security::Security;
use kyler_protocol::api::{
    gateway::{AuditRecord, CreateToken, Principal, Role, Scope},
    permissions::ApprovalStatus,
    questions::QuestionStatus,
    run::{CreateRunRequest, RunStatus},
};

pub(crate) struct Channels(Vec<Channel>);

struct Channel {
    bot: Adapter,
    store: ChannelStore,
    web_url: Url,
}

impl Channels {
    pub(crate) fn configured(path: &FsPath, settings: &Settings<'_>) -> Result<Option<Self>> {
        let adapters = Adapter::configured(settings)?;
        if adapters.is_empty() {
            return Ok(None);
        }
        let web_url = settings.web_origin()?;
        let mut channels = Vec::new();
        for bot in adapters {
            let store = ChannelStore::open(path, bot.namespace())?;
            channels.push(Channel {
                bot,
                store,
                web_url: web_url.clone(),
            });
        }
        Ok(Some(Self(channels)))
    }

    pub(crate) async fn run(&self, manager: &Arc<RunManager>) {
        futures::future::join_all(self.0.iter().map(|channel| channel.run(manager))).await;
    }

    fn get(&self, provider: &str) -> Option<&Channel> {
        self.0.iter().find(|channel| channel.bot.name() == provider)
    }
}

impl Channel {
    async fn run(&self, manager: &Arc<RunManager>) {
        if self.bot.name() == "api_server" {
            self.dispatch(manager).await;
        } else {
            tokio::join!(self.poll(), self.dispatch(manager), self.deliver(manager));
        }
    }

    async fn poll(&self) {
        if self.bot.streams() {
            let mut backoff = Duration::from_secs(2);
            loop {
                let started = std::time::Instant::now();
                if self.bot.listen(&self.store).await.is_err() {
                    tracing::warn!(
                        channel = self.bot.name(),
                        "Channel stream disconnected or unavailable"
                    );
                }
                if started.elapsed() >= Duration::from_secs(60) {
                    backoff = Duration::from_secs(2);
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
        let Adapter::Telegram(bot) = &self.bot else {
            return;
        };
        let mut offset = None;
        loop {
            match bot.poll(offset).await {
                Ok((next, messages)) => {
                    if self.store.ingest(messages).await.is_ok() {
                        offset = next;
                        continue;
                    }
                    tracing::warn!("Could not persist Telegram updates");
                }
                Err(error) => tracing::warn!("{error}"),
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    async fn dispatch(&self, manager: &Arc<RunManager>) {
        loop {
            match self.store.jobs().await {
                Ok(jobs) => {
                    for job in jobs {
                        if self.process(manager, &job).await.is_err() {
                            tracing::warn!(
                                channel = self.bot.name(),
                                "Could not advance channel job"
                            );
                        }
                    }
                }
                Err(_) => tracing::warn!(channel = self.bot.name(), "Could not read channel jobs"),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn deliver(&self, manager: &RunManager) {
        loop {
            match self.store.next_reply().await {
                Ok(Some(reply)) => {
                    let active = match reply.token {
                        Some(token) => {
                            manager
                                .security
                                .store
                                .active(token, chrono::Utc::now().timestamp_millis())
                                .await
                        }
                        None => Ok(true),
                    };
                    let retry = match active {
                        Ok(true) => !matches!(
                            self.bot
                                .send(reply.id, &reply.chat, &reply.text, reply.receipt.as_ref())
                                .await,
                            Ok(()) | Err(Some(400 | 403))
                        ),
                        Ok(false) => false,
                        Err(_) => true,
                    };
                    if self.store.delivery(reply.id, retry).await.is_ok() {
                        continue;
                    }
                    tracing::warn!(
                        channel = self.bot.name(),
                        "Could not persist channel delivery status"
                    );
                }
                Ok(None) => {}
                Err(_) => {
                    tracing::warn!(channel = self.bot.name(), "Could not read channel replies")
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn process(&self, manager: &Arc<RunManager>, job: &Job) -> Result<()> {
        let binding = &job.binding;
        let Some(principal) = principal(manager, binding).await? else {
            self.store
                .complete(job, String::new(), false, false)
                .await?;
            return Ok(());
        };
        let link = self.web_url.join(&format!("chat/{}", binding.session))?;
        let input = job.input.trim();
        if input == "/cancel" {
            let active = manager
                .registry
                .lock()
                .await
                .active
                .get(&binding.session)
                .cloned();
            let mut reply = "No channel run to cancel.";
            if let Some(id) = active
                && manager
                    .security
                    .store
                    .owns("run".into(), id.clone(), principal.id)
                    .await?
            {
                match manager.cancel(&id).await {
                    Ok(run) if run.status != RunStatus::Completed => {
                        reply = "Cancellation requested."
                    }
                    Ok(_) => {}
                    Err(error) if error.status.is_server_error() => {
                        anyhow::bail!("Could not cancel channel run")
                    }
                    Err(_) => {}
                }
            }
            return self.store.complete(job, reply.into(), false, false).await;
        }
        if input == "/new" {
            return self
                .store
                .complete(
                    job,
                    "New chat ready. Send a message to begin.".into(),
                    true,
                    false,
                )
                .await;
        }
        if input.starts_with('/') {
            return self
                .store
                .complete(
                    job,
                    "Commands: !start, !pair, !new, !cancel.".into(),
                    false,
                    false,
                )
                .await;
        }
        if let Some(run) = &job.run {
            let reply =
                match manager.entry(run).await {
                    Ok(entry) => {
                        let info = entry.info();
                        match info.status {
                            RunStatus::Completed => info
                                .output
                                .filter(|text| !text.trim().is_empty())
                                .unwrap_or_else(|| format!("Run completed.\n{link}")),
                            RunStatus::Failed => {
                                format!("Run failed. Open Kyler Web for details.\n{link}")
                            }
                            RunStatus::Cancelled => "Run cancelled.".into(),
                            _ => {
                                if !job.notified {
                                    let approval_pending =
                                        manager.approvals.as_ref().is_some_and(|approvals| {
                                            approvals.list(&binding.session).iter().any(|entry| {
                                                entry.run_id == *run
                                                    && entry.status == ApprovalStatus::Pending
                                            })
                                        });
                                    let question_pending =
                                        info.execution_id.as_ref().is_some_and(|execution| {
                                            manager.questions.list(&binding.session).iter().any(
                                                |entry| {
                                                    entry.execution_id == *execution
                                                        && entry.status == QuestionStatus::Pending
                                                },
                                            )
                                        });
                                    if approval_pending || question_pending {
                                        self.store.complete(
                                        job,
                                        format!("This run needs your input in Kyler Web.\n{link}"),
                                        false,
                                        true,
                                    ).await?;
                                    }
                                }
                                return Ok(());
                            }
                        }
                    }
                    Err(error) if error.status == StatusCode::NOT_FOUND => {
                        format!(
                            "Run no longer available. Check the conversation in Kyler Web.\n{link}"
                        )
                    }
                    Err(_) => anyhow::bail!("Could not read channel run"),
                };
            return self.store.complete(job, reply, false, false).await;
        }
        let request_id = match &self.bot {
            Adapter::Telegram(_) => format!("telegram:{}:{}", self.bot.namespace(), job.id),
            _ => format!("{}:{}", self.bot.name(), job.id),
        };
        if job.dispatched_at.is_some() {
            // An uncertain admission only resumes by recovering an existing run.
            // A missing key may mean that the run already aged out of retention.
            let key = format!(
                "{:x}",
                Sha256::digest(format!("{}\0{request_id}", principal.id))
            );
            let run = {
                let registry = manager.registry.lock().await;
                registry
                    .keys
                    .get(&(binding.session.clone(), key))
                    .filter(|(id, _)| registry.runs.contains_key(id))
                    .map(|(id, _)| id.clone())
            };
            return match run {
                Some(id) => self.store.set_run(job.id, id).await,
                None => {
                    self.store
                        .complete(
                            job,
                            format!("Unable to recover the submission. Check Kyler Web.\n{link}"),
                            false,
                            false,
                        )
                        .await
                }
            };
        }
        if let Err(error) = ensure_session(manager, &principal, binding).await {
            if error.status.is_server_error() {
                anyhow::bail!("Could not initialize channel session");
            }
            return self
                .store
                .complete(
                    job,
                    "The configured workspace is no longer accessible.".into(),
                    false,
                    false,
                )
                .await;
        }
        if manager.permits.available_permits() == 0
            || manager
                .registry
                .lock()
                .await
                .active
                .contains_key(&binding.session)
        {
            return Ok(());
        }
        let audit = manager
            .security
            .store
            .begin(AuditRecord {
                id: 0,
                request_id: uuid::Uuid::new_v4().to_string(),
                principal_id: Some(principal.id.clone()),
                token_id: principal.token_id.clone(),
                action: format!("CHANNEL {}.run", self.bot.name()),
                resource: Some(binding.session.clone()),
                started_at: chrono::Utc::now().timestamp_millis(),
                finished_at: None,
                status: None,
            })
            .await?;
        if !self.store.mark_dispatch(job.id).await? {
            return Ok(());
        }
        let result = manager
            .start_as(
                principal,
                binding.session.clone(),
                CreateRunRequest {
                    input: job.input.clone(),
                    command: None,
                    skills: Vec::new(),
                    attachment_ids: Vec::new(),
                    resume: false,
                    images: Vec::new(),
                    client_request_id: Some(request_id),
                },
            )
            .await;
        let status = result
            .as_ref()
            .map_or_else(|error| error.status, |_| StatusCode::ACCEPTED);
        manager
            .security
            .store
            .finish(
                audit,
                status.as_u16(),
                chrono::Utc::now().timestamp_millis(),
            )
            .await?;
        match result {
            Ok(run) => self.store.set_run(job.id, run.run_id).await,
            Err(error)
                if matches!(
                    error.body.code.as_str(),
                    "session_busy" | "capacity_exceeded" | "server_stopping"
                ) =>
            {
                self.store.clear_dispatch(job.id).await
            }
            Err(error) if error.status.is_server_error() => Ok(()),
            Err(_) => {
                self.store
                    .complete(
                        job,
                        format!("Kyler could not start this run.\n{link}"),
                        false,
                        false,
                    )
                    .await
            }
        }
    }
}

async fn principal(manager: &RunManager, binding: &Binding) -> Result<Option<Principal>> {
    let Some(record) = manager.security.store.token(binding.token.clone()).await? else {
        return Ok(None);
    };
    let token = record.info;
    if token.role != Role::Member
        || token.scope.workspaces.as_slice() != [binding.workspace.as_str()]
        || token.revoked_at.is_some()
        || token
            .expires_at
            .is_some_and(|expiry| expiry <= chrono::Utc::now().timestamp_millis())
    {
        return Ok(None);
    }
    Ok(Some(Principal {
        id: token.principal_id,
        token_id: Some(token.id),
        role: token.role,
        scope: token.scope,
    }))
}

async fn ensure_session(
    manager: &RunManager,
    principal: &Principal,
    binding: &Binding,
) -> ApiResult<()> {
    match manager.store.load(&binding.session).await {
        Ok(_) => return Ok(()),
        Err(error) if error.to_string() == "session not found" => {}
        Err(error) => return Err(ApiError::storage(error)),
    }
    let mut environment = manager
        .environment_for_create(Some(&binding.workspace))?
        .ok_or_else(crate::gateway::forbidden)?;
    if !Security::workspace_allowed(principal, Some(&environment.cwd)) {
        return Err(crate::gateway::forbidden());
    }
    environment.project = Some(kyler_core::environment::ProjectIdentity {
        root: environment.cwd.clone(),
    });
    let mut session = Session::new();
    session.session_id.clone_from(&binding.session);
    session.environment = Some(environment);
    manager.validate_session_environment(&session)?;
    manager
        .security
        .bind_session(principal, &session.session_id)
        .await
        .map_err(ApiError::storage)?;
    manager
        .store
        .save(&mut session)
        .await
        .map_err(ApiError::storage)
}

pub(crate) async fn catalog(State(state): State<AppState>) -> Json<Value> {
    let channels = CHANNELS.iter().map(|definition| {
        let enabled = state.channels.as_ref().is_some_and(|channels| channels.get(definition.id).is_some());
        json!({"id":definition.id,"name":definition.name,"enabled":enabled,"fields":definition.fields})
    }).collect::<Vec<_>>();
    Json(json!({"channels":channels}))
}

pub(crate) async fn status(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> ApiResult<Json<Value>> {
    let Some(channels) = state
        .channels
        .as_ref()
        .and_then(|channels| channels.get(&provider))
    else {
        return Ok(Json(json!({"enabled":false,"bindings":[]})));
    };
    let bindings: Vec<_> = channels
        .store
        .bindings()
        .await
        .map_err(ApiError::storage)?
        .into_iter()
        .map(|binding| json!({"chat_id":binding.chat.to_string(),"workspace":binding.workspace}))
        .collect();
    Ok(Json(json!({"enabled":true,"bindings":bindings})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PairRequest {
    workspace: String,
}

pub(crate) async fn pair(
    State(state): State<AppState>,
    Extension(owner): Extension<Principal>,
    Path(provider): Path<String>,
    Json(request): Json<PairRequest>,
) -> ApiResult<Json<Value>> {
    let channels = state
        .channels
        .as_ref()
        .and_then(|channels| channels.get(&provider))
        .ok_or_else(ApiError::missing)?;
    let environment = state
        .runs
        .environment_for_create(Some(&request.workspace))?
        .ok_or_else(|| ApiError::bad("A channel requires a workspace."))?;
    let workspace = environment
        .cwd
        .to_str()
        .ok_or_else(|| ApiError::bad("Workspace must be UTF-8."))?
        .to_owned();
    let issued = state
        .runs
        .security
        .issue(
            &owner,
            CreateToken {
                name: format!("{provider} channel"),
                role: Role::Member,
                scope: Scope {
                    workspaces: vec![workspace.clone()],
                    ..Scope::default()
                },
                expires_at: None,
            },
        )
        .await
        .map_err(ApiError::storage)?;
    let code = channels
        .store
        .pair(issued.info.id, workspace)
        .await
        .map_err(ApiError::storage)?;
    Ok(Json(json!({"code":code,"expires_in":300})))
}

pub(crate) async fn remove(
    State(state): State<AppState>,
    Extension(owner): Extension<Principal>,
    Path((provider, chat)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let channels = state
        .channels
        .as_ref()
        .and_then(|channels| channels.get(&provider))
        .ok_or_else(ApiError::missing)?;
    let binding = channels
        .store
        .bindings()
        .await
        .map_err(ApiError::storage)?
        .into_iter()
        .find(|binding| binding.chat == chat)
        .ok_or_else(ApiError::missing)?;
    state
        .runs
        .security
        .revoke(&owner, &binding.token)
        .await
        .map_err(ApiError::storage)?;
    channels
        .store
        .remove(chat, binding.token)
        .await
        .map_err(ApiError::storage)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn webhook(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let channels = state
        .channels
        .as_ref()
        .and_then(|channels| channels.get(&provider))
        .filter(|channel| channel.bot.accepts_webhook())
        .ok_or_else(ApiError::missing)?;
    if *state.runs.stopping.borrow() {
        return Err(ApiError::stopping());
    }
    let event = match &channels.bot {
        Adapter::WeComCallback(bot) => bot.webhook(query.as_deref().unwrap_or_default(), &body),
        _ => channels.bot.webhook(&headers, &body).await,
    }
    .map_err(|_| crate::gateway::forbidden())?;
    let (response, messages) = match event {
        Webhook::Challenge(challenge) => (
            Json(json!({"challenge":challenge})).into_response(),
            Vec::new(),
        ),
        Webhook::Contextual(message, context) => {
            channels
                .store
                .ingest_with_reply_context(message, context)
                .await
                .map_err(ApiError::storage)?;
            (Json(json!({})).into_response(), Vec::new())
        }
        Webhook::Messages(messages) => (Json(json!({})).into_response(), messages),
        Webhook::Ignore => (Json(json!({})).into_response(), Vec::new()),
        Webhook::Response(response, messages) => (Json(response).into_response(), messages),
        Webhook::Plain(response, messages) => (response.into_response(), messages),
        Webhook::Xml(response, messages) => (
            ([(header::CONTENT_TYPE, "application/xml")], response).into_response(),
            messages,
        ),
    };
    channels
        .store
        .ingest(messages)
        .await
        .map_err(ApiError::storage)?;
    Ok(response)
}

#[derive(Deserialize)]
pub(crate) struct SubscriptionQuery {
    #[serde(rename = "hub.mode")]
    mode: String,
    #[serde(rename = "hub.verify_token")]
    token: String,
    #[serde(rename = "hub.challenge")]
    challenge: String,
}

pub(crate) async fn verify_subscription(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    OriginalUri(uri): OriginalUri,
) -> ApiResult<String> {
    let channel = state
        .channels
        .as_ref()
        .and_then(|channels| channels.get(&provider))
        .ok_or_else(ApiError::missing)?;
    if let Adapter::WeComCallback(bot) = &channel.bot {
        return bot
            .verify(uri.query().unwrap_or_default())
            .map_err(|_| crate::gateway::forbidden());
    }
    if !matches!(channel.bot, Adapter::WhatsApp(_)) {
        return Err(ApiError::missing());
    }
    let Query(query) = Query::<SubscriptionQuery>::try_from_uri(&uri)
        .map_err(|_| ApiError::bad("Invalid callback query"))?;
    if !channel.bot.verify_subscription(&query.mode, &query.token) {
        return Err(crate::gateway::forbidden());
    }
    Ok(query.challenge)
}

fn pull_channel<'a>(state: &'a AppState, headers: &HeaderMap) -> ApiResult<&'a Channel> {
    let channel = state
        .channels
        .as_ref()
        .and_then(|channels| channels.get("api_server"))
        .ok_or_else(ApiError::missing)?;
    let credential = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !matches!(&channel.bot, Adapter::Bridge(bridge) if bridge.pull_allowed(credential)) {
        return Err(crate::gateway::forbidden());
    }
    Ok(channel)
}

pub(crate) async fn pull_reply(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let channel = pull_channel(&state, &headers)?;
    while let Some(reply) = channel
        .store
        .next_reply()
        .await
        .map_err(ApiError::storage)?
    {
        if let Some(token) = reply.token
            && !state
                .runs
                .security
                .store
                .active(token, chrono::Utc::now().timestamp_millis())
                .await
                .map_err(ApiError::storage)?
        {
            channel
                .store
                .delivery(reply.id, false)
                .await
                .map_err(ApiError::storage)?;
            continue;
        }
        return Ok((
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({
                "id": reply.id.to_string(), "chat": reply.chat, "text": reply.text
            })),
        )
            .into_response());
    }
    Ok((
        StatusCode::NO_CONTENT,
        [(header::CACHE_CONTROL, "no-store")],
    )
        .into_response())
}

pub(crate) async fn acknowledge_reply(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let channel = pull_channel(&state, &headers)?;
    channel
        .store
        .delivery(id, false)
        .await
        .map_err(ApiError::storage)?;
    Ok(StatusCode::NO_CONTENT)
}
