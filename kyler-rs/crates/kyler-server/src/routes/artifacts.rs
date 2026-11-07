//! Session-bound, immutable file downloads; workspace paths are never request parameters.
use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use axum::{
    Json,
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

use kyler_protocol::api::artifacts::ArtifactInfo;

pub async fn list(
    State(state): State<AppState>,
    Path(session): Path<String>,
) -> ApiResult<Json<Vec<ArtifactInfo>>> {
    valid_id(&session)?;
    state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    let store = state.runs.artifacts.clone();
    tokio::task::spawn_blocking(move || store.list(&session))
        .await
        .map_err(|_| ApiError::internal())?
        .map(Json)
        .map_err(ApiError::storage)
}
pub async fn download(
    State(state): State<AppState>,
    Path((session, id)): Path<(String, String)>,
) -> ApiResult<Response> {
    valid_id(&session)?;
    valid_id(&id)?;
    state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    let store = state.runs.artifacts.clone();
    let (info, bytes) = tokio::task::spawn_blocking(move || store.download(&session, &id))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::missing())?;
    Ok((
        [
            ("content-type", info.mime_type),
            (
                "content-disposition",
                format!(
                    "attachment; filename=\"download\"; filename*=UTF-8''{}",
                    utf8_percent_encode(&info.name, NON_ALPHANUMERIC)
                ),
            ),
            ("x-content-type-options", "nosniff".into()),
            ("cache-control", "private, no-store".into()),
        ],
        bytes,
    )
        .into_response())
}
