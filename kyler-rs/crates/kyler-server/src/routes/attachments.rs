use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};

use kyler_protocol::api::attachments::{AttachmentInfo, AttachmentKind};

#[derive(serde::Deserialize)]
pub struct UploadQuery {
    pub filename: String,
}
pub async fn upload(
    State(state): State<AppState>,
    Path(session): Path<String>,
    Query(query): Query<UploadQuery>,
    bytes: Bytes,
) -> ApiResult<(StatusCode, Json<AttachmentInfo>)> {
    valid_id(&session)?;
    state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    let store = state.runs.attachments.clone();
    let info = tokio::task::spawn_blocking(move || store.put(&session, &query.filename, &bytes))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|e| ApiError::bad(&e.to_string()))?;
    Ok((StatusCode::CREATED, Json(info)))
}
pub async fn list(
    State(state): State<AppState>,
    Path(session): Path<String>,
) -> ApiResult<Json<Vec<AttachmentInfo>>> {
    valid_id(&session)?;
    state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    let store = state.runs.attachments.clone();
    Ok(Json(
        tokio::task::spawn_blocking(move || store.list(&session))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(ApiError::storage)?,
    ))
}
pub async fn get(
    State(state): State<AppState>,
    Path((session, id)): Path<(String, String)>,
) -> ApiResult<Json<AttachmentInfo>> {
    valid_id(&session)?;
    valid_id(&id)?;
    state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    Ok(Json(
        state
            .runs
            .attachments
            .get(&session, &id)
            .map_err(|_| ApiError::missing())?,
    ))
}
pub async fn delete(
    State(state): State<AppState>,
    Path((session, id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    valid_id(&session)?;
    valid_id(&id)?;
    let snapshot = state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    if snapshot.events.iter().flat_map(|event|&event.content).any(|item|matches!(item,kyler_core::context::ContentItem::Attachment{attachment} if attachment.id==id)) {return Err(ApiError::bad("Referenced attachments are retained until the session is deleted."));}
    state
        .runs
        .attachments
        .delete(&session, &id)
        .map_err(|_| ApiError::missing())?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn content(
    State(state): State<AppState>,
    Path((session, id)): Path<(String, String)>,
) -> ApiResult<axum::response::Response> {
    valid_id(&session)?;
    valid_id(&id)?;
    state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    let store = state.runs.attachments.clone();
    let (info, bytes) = tokio::task::spawn_blocking(move || store.download(&session, &id))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::missing())?;
    let mime = if info.kind == AttachmentKind::Image {
        "image/png"
    } else {
        "application/octet-stream"
    };
    Ok((
        [
            ("content-type", mime),
            ("content-disposition", "attachment"),
            ("x-content-type-options", "nosniff"),
            ("cache-control", "private, no-store"),
        ],
        bytes,
    )
        .into_response())
}
