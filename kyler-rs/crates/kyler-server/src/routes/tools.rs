use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use axum::{
    Json,
    extract::{Path, State},
};

use kyler_protocol::api::tools::ToolDetails;

pub(super) use kyler_core::thread::projection::redact;
pub async fn details(
    State(state): State<AppState>,
    Path((session, call)): Path<(String, String)>,
) -> ApiResult<Json<ToolDetails>> {
    valid_id(&session)?;
    if call.is_empty() || call.len() > 512 {
        return Err(ApiError::bad("Invalid tool call ID"));
    }
    let snapshot = state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    kyler_core::thread::projection::tool_details(&snapshot, &call)
        .map(Json)
        .map_err(|_| ApiError::missing())
}
