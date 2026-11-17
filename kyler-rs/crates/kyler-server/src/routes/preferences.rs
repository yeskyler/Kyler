//! Shared single-user Web preferences; request patches never overwrite unrelated fields.
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};

use axum::{Json, extract::State};

use kyler_protocol::api::preferences::{PreferenceUpdate, UiPreferences};

pub async fn get(State(state): State<AppState>) -> ApiResult<Json<UiPreferences>> {
    let index = state.runs.store.catalog();
    tokio::task::spawn_blocking(move || index.preferences())
        .await
        .map_err(|_| ApiError::internal())?
        .map(Json)
        .map_err(ApiError::storage)
}
pub async fn update(
    State(state): State<AppState>,
    Json(request): Json<PreferenceUpdate>,
) -> ApiResult<Json<UiPreferences>> {
    if let Some(Some(id)) = &request.last_session_id {
        super::super::error::valid_id(id)?;
        if state.runs.is_draft(id).await? {
            return Err(ApiError::bad("Recent session is unavailable."));
        }
    }
    let index = state.runs.store.catalog();
    tokio::task::spawn_blocking(move || index.update_preferences(request))
        .await
        .map_err(|_| ApiError::internal())?
        .map(Json)
        .map_err(|_| {
            ApiError::bad("Could not update preferences; the selected session may be unavailable.")
        })
}
