use crate::{
    error::{ApiError, valid_id},
    state::AppState,
};

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};

use kyler_core::Agent;
use kyler_protocol::api::skills::{
    SaveSkillRequest, SkillAction, SkillActionRequest, SkillDetails, SkillInfo, SkillScope,
};

async fn binding(state: &AppState, id: &str) -> Result<Agent, ApiError> {
    valid_id(id)?;
    let session = state.runs.store.load(id).await.map_err(ApiError::storage)?;
    state.runs.agent_for_session(&session)
}
pub async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<SkillInfo>>, ApiError> {
    let agent = binding(&state, &id).await?;
    let Some(manager) = agent.skill_manager() else {
        return Ok(Json(Vec::new()));
    };
    let workspace = agent
        .workspace_environment()
        .map(|e| e.workspace_root.as_path());
    manager.list(workspace, false).await.map(Json).map_err(|_| {
        ApiError::bad("Unable to discover skills. Check configured directories and skill state.")
    })
}
pub async fn reload(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<SkillInfo>>, ApiError> {
    let agent = binding(&state, &id).await?;
    let manager = agent
        .skill_manager()
        .ok_or_else(|| ApiError::bad("Skills are disabled."))?;
    manager
        .list(
            agent
                .workspace_environment()
                .map(|e| e.workspace_root.as_path()),
            true,
        )
        .await
        .map(Json)
        .map_err(|_| {
            ApiError::bad("Skill reload failed. Check configured directories and package limits.")
        })
}
pub async fn details(
    State(state): State<AppState>,
    Path((id, skill)): Path<(String, String)>,
) -> Result<Json<SkillDetails>, ApiError> {
    let agent = binding(&state, &id).await?;
    let manager = agent
        .skill_manager()
        .ok_or_else(|| ApiError::bad("Skills are disabled."))?;
    manager
        .details(
            agent
                .workspace_environment()
                .map(|e| e.workspace_root.as_path()),
            &skill,
        )
        .await
        .map(Json)
        .map_err(|_| ApiError::bad("Skill unavailable or changed. Reload skills before reviewing."))
}
pub async fn action(
    State(state): State<AppState>,
    Path((id, skill, action)): Path<(String, String, String)>,
    Json(request): Json<SkillActionRequest>,
) -> Result<StatusCode, ApiError> {
    let action = match action.as_str() {
        "enable" => SkillAction::Enable,
        "disable" => SkillAction::Disable,
        _ => return Err(ApiError::bad("Unknown skill action.")),
    };
    let agent = binding(&state, &id).await?;
    let manager = agent
        .skill_manager()
        .ok_or_else(|| ApiError::bad("Skills are disabled."))?;
    manager.action(agent.workspace_environment().map(|e|e.workspace_root.as_path()),&skill,action,&request.fingerprint).await.map_err(|_|ApiError::bad("Skill changed, is shadowed, or approval could not be saved. Reload and review the current version."))?;
    Ok(StatusCode::NO_CONTENT)
}

/// User initiated package creation; tool approval remains on the agent tool path.
pub async fn create(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SaveSkillRequest>,
) -> Result<(StatusCode, Json<SkillInfo>), ApiError> {
    if request.scope != SkillScope::User || request.expected_fingerprint.is_some() {
        return Err(ApiError::bad("This endpoint creates new user skills only."));
    }
    let agent = binding(&state, &id).await?;
    let manager = agent
        .skill_manager()
        .ok_or_else(|| ApiError::bad("Skills are disabled."))?;
    let info = manager.save(agent.workspace_environment().map(|e| e.workspace_root.as_path()), request).await
        .map_err(|_| ApiError::bad("Cannot save skill. Check the skill name, SKILL.md metadata, package limits, and whether the skill already exists."))?;
    Ok((StatusCode::CREATED, Json(info)))
}
