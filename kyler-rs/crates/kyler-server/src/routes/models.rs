use crate::{error::ApiError, state::AppState};

use axum::{
    Extension, Json,
    extract::{Path, State},
};

use kyler_protocol::api::models::{
    ModelCatalog, ModelConnectionSettings, ModelConnectionUpdate, ModelSelection, SessionModel,
};

pub async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<kyler_protocol::api::gateway::Principal>,
) -> Result<Json<ModelCatalog>, ApiError> {
    let mut catalog = state.runs.model_catalog()?;
    catalog.models.retain(|model| {
        kyler_gateway::security::Security::model_allowed(&principal, &model.profile)
    });
    catalog.providers.retain(|provider| {
        catalog
            .models
            .iter()
            .any(|model| model.provider == provider.provider)
    });
    if !kyler_gateway::security::Security::model_allowed(&principal, &catalog.default_profile) {
        catalog.default_profile = catalog
            .models
            .first()
            .map(|model| model.profile.clone())
            .unwrap_or_default();
    }
    Ok(Json(catalog))
}
pub async fn reload(State(state): State<AppState>) -> Result<Json<ModelCatalog>, ApiError> {
    state.runs.reload_models().await.map(Json)
}
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionModel>, ApiError> {
    state.runs.session_model(&id, None).await.map(Json)
}
pub async fn select(
    State(state): State<AppState>,
    Extension(principal): Extension<kyler_protocol::api::gateway::Principal>,
    Path(id): Path<String>,
    Json(selection): Json<ModelSelection>,
) -> Result<Json<SessionModel>, ApiError> {
    let catalog = state.runs.model_catalog()?;
    let profile = selection
        .profile
        .as_deref()
        .unwrap_or(&catalog.default_profile);
    if !kyler_gateway::security::Security::model_allowed(&principal, profile) {
        return Err(crate::gateway::forbidden());
    }
    state
        .runs
        .session_model(&id, Some(selection))
        .await
        .map(Json)
}

pub async fn connection(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ModelConnectionSettings>, ApiError> {
    state.runs.model_connection(&id).await.map(Json)
}
pub async fn update_connection(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ModelConnectionUpdate>,
) -> Result<Json<ModelConnectionSettings>, ApiError> {
    state
        .runs
        .update_model_connection(&id, request)
        .await
        .map(Json)
}

pub async fn global_connection(
    State(state): State<AppState>,
) -> Result<Json<ModelConnectionSettings>, ApiError> {
    state.runs.global_model_connection().await.map(Json)
}
pub async fn update_global_connection(
    State(state): State<AppState>,
    Json(request): Json<ModelConnectionUpdate>,
) -> Result<Json<ModelConnectionSettings>, ApiError> {
    state
        .runs
        .update_global_model_connection(request)
        .await
        .map(Json)
}

pub async fn thinking(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<kyler_protocol::api::models::ThinkingState>, ApiError> {
    state.runs.session_thinking(&id, None).await.map(Json)
}
pub async fn select_thinking(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(selection): Json<kyler_protocol::api::models::ThinkingSelection>,
) -> Result<Json<kyler_protocol::api::models::ThinkingState>, ApiError> {
    let level = selection.selected_level().map_err(ApiError::bad)?;
    state
        .runs
        .session_thinking_level(&id, level)
        .await
        .map(Json)
}
