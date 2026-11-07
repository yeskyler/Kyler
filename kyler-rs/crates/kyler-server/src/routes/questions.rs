//! Session-scoped question recovery and idempotent user replies.
use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
};
use kyler_protocol::api::questions::{QuestionRecord, QuestionResponse};

pub async fn list(
    State(state): State<AppState>,
    Path(session): Path<String>,
) -> ApiResult<Json<Vec<QuestionRecord>>> {
    valid_id(&session)?;
    state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    Ok(Json(state.runs.questions.list(&session)))
}
pub async fn respond(
    State(state): State<AppState>,
    Path((session, id)): Path<(String, String)>,
    Json(response): Json<QuestionResponse>,
) -> ApiResult<Json<QuestionRecord>> {
    valid_id(&session)?;
    valid_id(&id)?;
    state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    if !state
        .runs
        .questions
        .list(&session)
        .iter()
        .any(|record| record.id == id)
    {
        return Err(ApiError::missing());
    }
    state
        .runs
        .questions
        .respond(&session, &id, &response)
        .map(Json)
        .map_err(|error| ApiError::bad(&error.to_string()))
}
