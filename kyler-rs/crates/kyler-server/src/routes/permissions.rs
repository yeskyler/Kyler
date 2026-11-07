use crate::{error::ApiError, state::AppState};

use axum::{
    Json,
    extract::{Path, State},
};

use kyler_protocol::api::permissions::{
    ApprovalRecord, ApprovalResponse, PermissionSelection, PermissionState,
};

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PermissionState>, ApiError> {
    state.runs.permission_state(&id, None).await.map(Json)
}
pub async fn select(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(selection): Json<PermissionSelection>,
) -> Result<Json<PermissionState>, ApiError> {
    state
        .runs
        .permission_state(&id, Some(selection))
        .await
        .map(Json)
}
pub async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<ApprovalRecord>>, ApiError> {
    state.runs.approval_list(&id).await.map(Json)
}
pub async fn decide(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(response): Json<ApprovalResponse>,
) -> Result<Json<ApprovalRecord>, ApiError> {
    state.runs.approval_decide(&id, &response).map(Json)
}
