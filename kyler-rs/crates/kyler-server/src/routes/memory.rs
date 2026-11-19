use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};

use serde::Deserialize;

use kyler_core::memory::{MemoryEntry, MemoryStore};
use kyler_protocol::api::memory::{
    MemoryReviewDetails, MemoryReviewPage, MemoryReviewRequest, MemoryStatus,
};

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

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryPage {
    pub cursor: usize,
    pub limit: usize,
    pub status: Option<MemoryStatus>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryLookup {
    pub key: String,
}

impl Default for MemoryPage {
    fn default() -> Self {
        Self {
            cursor: 0,
            limit: 20,
            status: None,
        }
    }
}
impl MemoryPage {
    fn end(&self, total: usize) -> ApiResult<usize> {
        if !(1..=50).contains(&self.limit) || self.cursor > total {
            return Err(ApiError::bad("Invalid memory page cursor or limit."));
        }
        Ok(self.cursor.saturating_add(self.limit).min(total))
    }
}
async fn review_scope(state: &AppState, id: &str) -> ApiResult<String> {
    valid_id(id)?;
    let session = state.runs.store.load(id).await.map_err(ApiError::storage)?;
    Ok(MemoryStore::scope(&session))
}
fn changed() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "memory_changed",
        "Memory content or scope changed. Reload and review the current version.",
    )
}

pub async fn memories(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(page): Query<MemoryPage>,
) -> ApiResult<Json<MemoryReviewPage>> {
    let scope = review_scope(&state, &id).await?;
    let store = state.runs.memories.clone();
    let bound = scope.clone();
    let entries = tokio::task::spawn_blocking(move || store.list(&bound))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(ApiError::storage)?;
    let entries = entries
        .into_iter()
        .filter(|entry| page.status.is_none_or(|status| entry.status == status))
        .collect::<Vec<_>>();
    let end = page.end(entries.len())?;
    Ok(Json(MemoryReviewPage {
        scope,
        total: entries.len(),
        next_cursor: (end < entries.len()).then_some(end),
        entries: entries[page.cursor..end].to_vec(),
    }))
}
pub async fn details(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(lookup): Query<MemoryLookup>,
) -> ApiResult<Json<MemoryReviewDetails>> {
    let key = lookup.key;
    let scope = review_scope(&state, &id).await?;
    let store = state.runs.memories.clone();
    let bound = scope.clone();
    let entry = tokio::task::spawn_blocking(move || store.list(&bound))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(ApiError::storage)?
        .into_iter()
        .find(|entry| entry.key == key)
        .ok_or_else(ApiError::missing)?;
    Ok(Json(MemoryReviewDetails { scope, entry }))
}
pub async fn review_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<MemoryReviewRequest>,
) -> ApiResult<Json<MemoryReviewDetails>> {
    let scope = review_scope(&state, &id).await?;
    if request.expected_scope != scope {
        return Err(changed());
    }
    if request.status == MemoryStatus::Candidate {
        return Err(ApiError::bad("Choose active, rejected or invalid."));
    }
    let store = state.runs.memories.clone();
    let bound = scope.clone();
    let entry = tokio::task::spawn_blocking(move || {
        store.review(
            &bound,
            &request.key,
            request.expected_revision,
            request.status,
        )
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|error| match error.to_string().as_str() {
        "Memory revision conflict" => changed(),
        "Memory not found" => ApiError::missing(),
        _ => ApiError::bad(
            "Memory cannot be activated if expired or sensitive. Reload before reviewing.",
        ),
    })?;
    Ok(Json(MemoryReviewDetails { scope, entry }))
}
