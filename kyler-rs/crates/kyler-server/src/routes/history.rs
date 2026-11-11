//! HTTP pagination around the shared core history projection.
use super::sessions::{Pagination, page};
use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use axum::{
    Json,
    extract::{Path, Query, State, rejection::QueryRejection},
};

use kyler_protocol::api::history::HistoryPage;

pub async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
    query: Result<Query<Pagination>, QueryRejection>,
) -> ApiResult<Json<HistoryPage>> {
    valid_id(&id)?;
    let (cursor, limit) = page(query)?;
    let session = state
        .runs
        .store
        .load(&id)
        .await
        .map_err(ApiError::storage)?;
    kyler_core::thread::projection::history(&session, cursor, limit)
        .map(Json)
        .map_err(|error| ApiError::bad(&error.to_string()))
}
