use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use axum::{
    Json,
    extract::{Path, Query, State},
};

use kyler_core::memory::{MemoryEntry, MemoryStore};

#[derive(serde::Deserialize, Default)]
pub struct Search {
    #[serde(default)]
    pub query: String,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Save {
    pub key: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub expected_revision: Option<u64>,
}
pub async fn list(
    State(state): State<AppState>,
    Path(session): Path<String>,
    Query(query): Query<Search>,
) -> ApiResult<Json<Vec<MemoryEntry>>> {
    valid_id(&session)?;
    let snapshot = state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    let scope = MemoryStore::scope(&snapshot);
    let store = state.runs.memories.clone();
    Ok(Json(
        tokio::task::spawn_blocking(move || store.search(&scope, &query.query))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(ApiError::storage)?,
    ))
}
pub async fn save(
    State(state): State<AppState>,
    Path(session): Path<String>,
    Json(args): Json<Save>,
) -> ApiResult<Json<MemoryEntry>> {
    valid_id(&session)?;
    let snapshot = state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    let scope = MemoryStore::scope(&snapshot);
    let store = state.runs.memories.clone();
    Ok(Json(
        tokio::task::spawn_blocking(move || {
            store.put(
                &scope,
                &args.key,
                &args.content,
                args.tags,
                args.expected_revision,
                &session,
            )
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|error| ApiError::bad(&error.to_string()))?,
    ))
}
pub async fn delete(
    State(state): State<AppState>,
    Path((session, key)): Path<(String, String)>,
) -> ApiResult<axum::http::StatusCode> {
    valid_id(&session)?;
    let snapshot = state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    let scope = MemoryStore::scope(&snapshot);
    let store = state.runs.memories.clone();
    tokio::task::spawn_blocking(move || store.delete(&scope, &key))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(ApiError::storage)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
