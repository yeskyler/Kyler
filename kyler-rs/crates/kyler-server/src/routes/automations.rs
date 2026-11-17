use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};

use kyler_protocol::api::automations::{
    RunTaskRequest, SaveTaskRequest, SchedulePreview, SchedulePreviewRequest, ScheduledRun,
    ScheduledRunPage, ScheduledTask, SetTaskEnabled,
};
use kyler_scheduler::{Error, Scheduler};

fn scheduler(state: &AppState) -> ApiResult<Arc<Scheduler>> {
    state.scheduler.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "scheduler_unavailable",
            "Automations are unavailable for this embedded service.",
        )
    })
}
fn error(error: anyhow::Error) -> ApiError {
    match error.downcast_ref::<Error>() {
        Some(Error::Invalid(message)) => ApiError::bad(message),
        Some(Error::Conflict(message)) => {
            ApiError::new(StatusCode::CONFLICT, "automation_conflict", message)
        }
        Some(Error::NotFound) => ApiError::missing(),
        Some(Error::LeaseLost) => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "scheduler_lease_lost",
            "Scheduler ownership changed. Retry after the service recovers.",
        ),
        None => {
            tracing::error!("automation storage operation failed");
            ApiError::internal()
        }
    }
}
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<ScheduledTask>>> {
    scheduler(&state)?
        .store
        .tasks()
        .await
        .map(Json)
        .map_err(error)
}
pub async fn status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let scheduler = scheduler(&state)?;
    Ok(Json(json!({"running":scheduler.is_running()})))
}
pub async fn create(
    State(state): State<AppState>,
    Json(request): Json<SaveTaskRequest>,
) -> ApiResult<(StatusCode, Json<ScheduledTask>)> {
    scheduler(&state)?
        .save_task(None, request)
        .await
        .map(|task| (StatusCode::CREATED, Json(task)))
        .map_err(error)
}
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ScheduledTask>> {
    valid_id(&id)?;
    scheduler(&state)?
        .store
        .task(&id)
        .await
        .map(Json)
        .map_err(error)
}
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SaveTaskRequest>,
) -> ApiResult<Json<ScheduledTask>> {
    valid_id(&id)?;
    scheduler(&state)?
        .save_task(Some(&id), request)
        .await
        .map(Json)
        .map_err(error)
}
pub async fn enabled(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SetTaskEnabled>,
) -> ApiResult<Json<ScheduledTask>> {
    valid_id(&id)?;
    scheduler(&state)?
        .store
        .set_enabled(
            &id,
            request.expected_revision,
            request.enabled,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .map(Json)
        .map_err(error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteQuery {
    revision: u64,
}
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DeleteQuery>,
) -> ApiResult<StatusCode> {
    valid_id(&id)?;
    scheduler(&state)?
        .store
        .delete(&id, query.revision, chrono::Utc::now().timestamp_millis())
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(error)
}
pub async fn preview(
    State(state): State<AppState>,
    Json(request): Json<SchedulePreviewRequest>,
) -> ApiResult<Json<SchedulePreview>> {
    scheduler(&state)?;
    let values = tokio::task::spawn_blocking(move || {
        kyler_scheduler::schedule::preview(
            &request.cron,
            &request.timezone,
            chrono::Utc::now().timestamp_millis(),
        )
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(error)?;
    Ok(Json(SchedulePreview {
        occurrences: values,
    }))
}
pub async fn run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<RunTaskRequest>,
) -> ApiResult<(StatusCode, Json<ScheduledRun>)> {
    valid_id(&id)?;
    valid_id(&request.request_id)?;
    let scheduler = scheduler(&state)?;
    if !scheduler.is_running() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "scheduler_stopped",
            "Scheduler is not running.",
        ));
    }
    scheduler
        .store
        .enqueue(
            &id,
            &request.request_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .map(|run| (StatusCode::ACCEPTED, Json(run)))
        .map_err(error)
}
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct RunQuery {
    cursor: Option<String>,
    limit: Option<usize>,
}
pub async fn runs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<RunQuery>,
) -> ApiResult<Json<ScheduledRunPage>> {
    valid_id(&id)?;
    if let Some(cursor) = &query.cursor {
        valid_id(cursor)?;
    }
    let limit = query.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(ApiError::bad("limit must be in 1..=200."));
    }
    scheduler(&state)?
        .store
        .runs(&id, query.cursor.as_deref(), limit)
        .await
        .map(Json)
        .map_err(error)
}
pub async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ScheduledRun>> {
    valid_id(&id)?;
    scheduler(&state)?
        .store
        .cancel(&id, chrono::Utc::now().timestamp_millis())
        .await
        .map(Json)
        .map_err(error)
}
