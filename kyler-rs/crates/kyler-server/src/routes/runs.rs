use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};

use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
};

use kyler_protocol::api::run::{CreateRunRequest, RunAccepted, RunInfo};

pub async fn create(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<CreateRunRequest>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<RunAccepted>)> {
    let Json(request) = body.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_too_large",
                "The request exceeds the body limit.",
            )
        } else {
            ApiError::bad("A valid JSON run request is required.")
        }
    })?;
    Ok((
        StatusCode::ACCEPTED,
        Json(state.runs.start(id, request).await?),
    ))
}
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<RunInfo>> {
    Ok(Json(state.runs.entry(&id).await?.info()))
}
pub async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<RunInfo>> {
    Ok(Json(state.runs.cancel(&id).await?))
}
