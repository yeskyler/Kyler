use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};

use kyler_protocol::api::workspaces::{
    SaveWorkspaceRequest, WorkspaceFiles, WorkspaceInfo, WorkspaceInput,
};

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<WorkspaceInfo>>> {
    state.runs.list_workspaces().await.map(Json)
}
pub async fn save(
    State(state): State<AppState>,
    Json(request): Json<SaveWorkspaceRequest>,
) -> ApiResult<Json<WorkspaceInfo>> {
    state.runs.save_workspace(request).await.map(Json)
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    state.runs.delete_workspace(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Inputs and retained outputs belong only to the requested session.
pub async fn files(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<WorkspaceFiles>> {
    valid_id(&id)?;
    let current = state
        .runs
        .store
        .load(&id)
        .await
        .map_err(ApiError::storage)?;
    let times = current
        .executions
        .iter()
        .chain(current.execution.iter())
        .map(|execution| (execution.execution_id.clone(), execution.started_at))
        .collect::<HashMap<_, _>>();
    let owners = vec![(current.session_id, times, current.updated_at)];
    let attachments = state.runs.attachments.clone();
    let artifacts = state.runs.artifacts.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let mut result = WorkspaceFiles::default();
        let mut times = HashMap::new();
        for (session, executions, updated_at) in owners {
            result
                .inputs
                .extend(
                    attachments
                        .list(&session)?
                        .into_iter()
                        .map(|attachment| WorkspaceInput {
                            session_id: session.clone(),
                            attachment,
                        }),
                );
            for artifact in artifacts.list(&session)? {
                times.insert(
                    artifact.id.clone(),
                    executions
                        .get(&artifact.execution_id)
                        .copied()
                        .unwrap_or(updated_at),
                );
                result.outputs.push(artifact);
            }
        }
        result.inputs.sort_by(|a, b| {
            b.attachment
                .created_at
                .cmp(&a.attachment.created_at)
                .then_with(|| a.attachment.id.cmp(&b.attachment.id))
        });
        result.outputs.sort_by(|a, b| {
            times[&b.id]
                .cmp(&times[&a.id])
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(Json(result))
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(ApiError::storage)
}
