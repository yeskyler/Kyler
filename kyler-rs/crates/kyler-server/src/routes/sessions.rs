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

use kyler_core::{ExecutionStatus, Session, context::ContentItem};
use kyler_protocol::api::{
    SessionUsage,
    run::RunStatus,
    session::{CreateSessionRequest, Message, MessagePage, SessionPage, SessionSummary},
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
    let title = session
        .events
        .iter()
        .flat_map(|event| &event.content)
        .find_map(|item| {
            if let ContentItem::Message { role, content } = item {
                if role.as_str() == "user" {
                    let title = content.split_whitespace().collect::<Vec<_>>().join(" ");
                    return (!title.is_empty()).then(|| title.chars().take(80).collect());
                }
            }
            None
        });
    SessionSummary {
        title,
        draft: false,
        workspace_root: session.environment.as_ref().map(|e| {
            e.project_root
                .as_ref()
                .unwrap_or(&e.workspace_root)
                .to_string_lossy()
                .into_owned()
        }),
        working_directory: session
            .environment
            .as_ref()
            .map(|e| e.working_directory.to_string_lossy().into_owned()),
        session_id: session.session_id,
        created_at: session.created_at,
        updated_at: session.updated_at,
        event_count: session.events.len(),
        execution_count: session.executions.len() + usize::from(session.execution.is_some()),
        status: session.execution.map(|e| match e.status {
            ExecutionStatus::Running => RunStatus::Running,
            ExecutionStatus::Completed => RunStatus::Completed,
            ExecutionStatus::Failed => RunStatus::Failed,
            ExecutionStatus::Cancelled => RunStatus::Cancelled,
        }),
    }
}
pub async fn create(
    State(state): State<AppState>,
    payload: Result<Option<Json<CreateSessionRequest>>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<(StatusCode, Json<SessionSummary>)> {
    let request = payload
        .map_err(|_| ApiError::bad("Invalid session creation request."))?
        .map(|Json(value)| value)
        .unwrap_or_default();
    let environment = state
        .runs
        .environment_for_create(request.workspace_root.as_deref())?;
    let mut session = Session::new();
    session.environment = environment;
    if request.workspace_mode == kyler_protocol::api::session::WorkspaceMode::Project {
        session
            .environment
            .as_mut()
            .ok_or_else(|| ApiError::bad("Project execution requires an authorized workspace."))?
            .workspace_mode = request.workspace_mode;
    }
    state.runs.isolate_session(&mut session).await?;
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
    let mut sessions = state.runs.store.list().await.map_err(ApiError::storage)?;
    let drafts = state.runs.draft_sessions().await?;
    sessions.retain(|session| !drafts.contains(&session.session_id));
    if cursor > sessions.len() {
        return Err(ApiError::bad("Pagination cursor is out of range."));
    }
    let end = cursor.saturating_add(limit).min(sessions.len());
    let next_cursor = (end < sessions.len()).then_some(end);
    let mut items = Vec::new();
    for info in sessions.into_iter().skip(cursor).take(limit) {
        match state.runs.store.load(&info.session_id).await {
            Ok(session) => items.push(summary(session)),
            Err(error) if error.to_string() == "session not found" => continue,
            Err(error) => return Err(ApiError::storage(error)),
        }
    }
    apply_run_statuses(&state, &mut items).await;
    Ok(Json(SessionPage {
        sessions: items,
        next_cursor,
    }))
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
