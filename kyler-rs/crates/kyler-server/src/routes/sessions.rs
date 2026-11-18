use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, Query, State, rejection::QueryRejection},
    http::StatusCode,
};
use serde::Deserialize;

use kyler_core::{Session, context::ContentItem};
use kyler_protocol::api::{
    SessionUsage,
    session::{
        ActiveSessionRun, CreateSessionRequest, Message, MessagePage, SessionPage, SessionSummary,
    },
};

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Pagination {
    cursor: Option<usize>,
    limit: Option<usize>,
}
pub(super) fn page(query: Result<Query<Pagination>, QueryRejection>) -> ApiResult<(usize, usize)> {
    let Query(query) = query.map_err(|_| ApiError::bad("Invalid pagination query."))?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=500).contains(&limit) {
        return Err(ApiError::bad("limit must be in 1..=500."));
    }
    Ok((query.cursor.unwrap_or(0), limit))
}
pub fn summary(session: Session) -> SessionSummary {
    kyler_core::session::store::catalog::summary(&session)
}
pub async fn create(
    State(state): State<AppState>,
    payload: Result<Option<Json<CreateSessionRequest>>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<(StatusCode, Json<SessionSummary>)> {
    let request = payload
        .map_err(|_| ApiError::bad("Invalid session creation request."))?
        .map(|Json(value)| value)
        .unwrap_or_default();
    let environment = state.runs.environment_for_create(request.cwd.as_deref())?;
    let mut session = Session::new();
    session.environment = environment;
    if let Some(environment) = session.environment.as_mut() {
        if let Some(project) = state
            .runs
            .list_workspaces()
            .await?
            .into_iter()
            .filter(|project| environment.cwd.starts_with(&project.path))
            .max_by_key(|project| project.path.len())
        {
            environment.project = Some(kyler_core::environment::ProjectIdentity {
                root: project.path.into(),
            });
        }
    }
    let mode = request
        .workspace_mode
        .unwrap_or(kyler_protocol::api::session::WorkspaceMode::Project);
    if mode == kyler_protocol::api::session::WorkspaceMode::Isolated {
        state.runs.allocate_session_directory(&mut session)?;
    }
    state.runs.validate_session_environment(&session)?;
    if request.draft {
        state.runs.mark_draft(&session.session_id).await?;
    }
    if let Err(error) = state.runs.store.save(&mut session).await {
        if request.draft {
            let _ = state.runs.publish_session(&session.session_id).await;
        }
        return Err(ApiError::storage(error));
    }
    let mut item = summary(session);
    item.draft = request.draft;
    Ok((StatusCode::CREATED, Json(item)))
}
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SessionSummary>> {
    valid_id(&id)?;
    let mut item = summary(
        state
            .runs
            .store
            .load(&id)
            .await
            .map_err(ApiError::storage)?,
    );
    item.draft = state.runs.is_draft(&id).await?;
    apply_run_statuses(&state, std::slice::from_mut(&mut item)).await;
    Ok(Json(item))
}
pub async fn list(
    State(state): State<AppState>,
    query: Result<Query<Pagination>, QueryRejection>,
) -> ApiResult<Json<SessionPage>> {
    let (cursor, limit) = page(query)?;
    let mut page = state
        .runs
        .store
        .page(cursor, limit)
        .await
        .map_err(|error| {
            if error.to_string() == "Pagination cursor is out of range." {
                ApiError::bad("Pagination cursor is out of range.")
            } else {
                ApiError::storage(error)
            }
        })?;
    apply_run_statuses(&state, &mut page.sessions).await;
    Ok(Json(page))
}

// Admission and cancellation are visible before the next session checkpoint.
// Terminal run status also preserves failures such as a timeout whose execution
// checkpoint records cancellation.
async fn apply_run_statuses(state: &AppState, sessions: &mut [SessionSummary]) {
    let registry = state.runs.registry.lock().await;
    let mut latest = HashMap::new();
    for entry in registry.runs.values() {
        let info = entry.info();
        let current = latest
            .entry(info.session_id)
            .or_insert((info.created_at, info.status));
        if info.created_at > current.0 {
            *current = (info.created_at, info.status);
        }
    }
    for session in sessions {
        if let Some(run) = registry
            .active
            .get(&session.session_id)
            .and_then(|id| registry.runs.get(id))
        {
            session.status = Some(run.info().status);
        } else if let Some((_, status)) = latest.get(&session.session_id) {
            session.status = Some(*status);
        }
    }
}

pub async fn messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    query: Result<Query<Pagination>, QueryRejection>,
) -> ApiResult<Json<MessagePage>> {
    valid_id(&id)?;
    let (cursor, limit) = page(query)?;
    let session = state
        .runs
        .store
        .load(&id)
        .await
        .map_err(ApiError::storage)?;
    if cursor > session.events.len() {
        return Err(ApiError::bad("Pagination cursor is out of range."));
    }
    let end = cursor.saturating_add(limit).min(session.events.len());
    let mut messages = Vec::new();
    for event in &session.events[cursor..end] {
        for item in &event.content {
            if let ContentItem::Message { role, content } = item {
                messages.push(Message {
                    id: event.id.clone(),
                    execution_id: event.execution_id.clone(),
                    timestamp: event.timestamp,
                    role: role.as_str().into(),
                    content: std::iter::once(content.clone())
                        .filter(|text| !text.is_empty())
                        .chain(event.content.iter().filter_map(|item| match item {
                            ContentItem::Attachment { attachment } => {
                                Some(format!("[Attachment: {}]", attachment.name))
                            }
                            ContentItem::Image { image } => {
                                Some(format!("[Image: {}]", image.name()))
                            }
                            _ => None,
                        }))
                        .collect::<Vec<_>>()
                        .join("\n"),
                });
            }
        }
    }
    Ok(Json(MessagePage {
        messages,
        next_cursor: (end < session.events.len()).then_some(end),
    }))
}
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    state.runs.delete_session(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn usage(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SessionUsage>> {
    valid_id(&id)?;
    let session = state
        .runs
        .store
        .load(&id)
        .await
        .map_err(ApiError::storage)?;
    Ok(Json(SessionUsage {
        prompt_tokens: u64::from(session.usage.prompt_tokens)
            + u64::from(session.subagent_usage.prompt_tokens),
        completion_tokens: u64::from(session.usage.completion_tokens)
            + u64::from(session.subagent_usage.completion_tokens),
        total_tokens: u64::from(session.usage.total_tokens)
            + u64::from(session.subagent_usage.total_tokens),
    }))
}

pub async fn active_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Option<ActiveSessionRun>>> {
    valid_id(&id)?;
    // Authenticate the session's existence before exposing run metadata.
    state
        .runs
        .store
        .load(&id)
        .await
        .map_err(ApiError::storage)?;
    let registry = state.runs.registry.lock().await;
    let value = registry
        .active
        .get(&id)
        .and_then(|run| registry.runs.get(run))
        .map(|entry| ActiveSessionRun {
            run: entry.info(),
            input: entry
                .input
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or_default(),
        });
    Ok(Json(value))
}
