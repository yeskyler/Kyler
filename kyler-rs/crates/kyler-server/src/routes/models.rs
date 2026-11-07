use crate::{error::ApiError, state::AppState};

use axum::{
    Json,
    extract::{Path, State},
};

use kyler_protocol::api::models::{
    ModelCatalog, ModelConnectionSettings, ModelConnectionUpdate, ModelSelection, SessionModel,
};

pub async fn list(State(state): State<AppState>) -> Result<Json<ModelCatalog>, ApiError> {
    state.runs.model_catalog().map(Json)
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
    Path(id): Path<String>,
    Json(selection): Json<ModelSelection>,
) -> Result<Json<SessionModel>, ApiError> {
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
