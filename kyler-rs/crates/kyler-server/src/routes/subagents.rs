//! Administrative delegation configuration, separate from session task operations.
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};

use axum::{Extension, Json, extract::State};

use kyler_protocol::api::{
    gateway::Principal,
    subagents::{SubagentSettingsResponse, SubagentSettingsUpdate},
};

fn authorize(principal: &Principal) -> ApiResult<()> {
    if !principal.role.administrative() {
        return Err(crate::gateway::forbidden());
    }
    Ok(())
}
pub async fn get(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<SubagentSettingsResponse>, ApiError> {
    authorize(&principal)?;
    state.runs.subagent_settings().await.map(Json)
}
pub async fn update(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<SubagentSettingsUpdate>,
) -> Result<Json<SubagentSettingsResponse>, ApiError> {
    authorize(&principal)?;
    state.runs.update_subagent_settings(request).await.map(Json)
}
