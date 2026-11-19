use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};

use kyler_core::{Agent, memory::MemoryStore};
use kyler_protocol::api::skills::{
    DraftReviewDetails, DraftReviewPage, PublishDraftRequest, SaveSkillRequest, SkillAction,
    SkillActionRequest, SkillDetails, SkillDraftDetails, SkillInfo, SkillScope, SkillSource,
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
    let workspace = agent.workspace_environment().map(|e| e.cwd.as_path());
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
        .list(agent.workspace_environment().map(|e| e.cwd.as_path()), true)
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
            agent.workspace_environment().map(|e| e.cwd.as_path()),
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
    manager.action(agent.workspace_environment().map(|e|e.cwd.as_path()),&skill,action,&request.fingerprint).await.map_err(|_|ApiError::bad("Skill changed, is shadowed, or approval could not be saved. Reload and review the current version."))?;
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
    let info = manager.save(agent.workspace_environment().map(|e| e.cwd.as_path()), request).await
        .map_err(|_| ApiError::bad("Cannot save skill. Check the skill name, SKILL.md metadata, package limits, and whether the skill already exists."))?;
    Ok((StatusCode::CREATED, Json(info)))
}

#[derive(serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DraftPage {
    pub cursor: usize,
    pub limit: usize,
}
impl Default for DraftPage {
    fn default() -> Self {
        Self {
            cursor: 0,
            limit: 20,
        }
    }
}
impl DraftPage {
    fn end(&self, total: usize) -> Result<usize, ApiError> {
        if !(1..=50).contains(&self.limit) || self.cursor > total {
            return Err(ApiError::bad("Invalid draft page cursor or limit."));
        }
        Ok(self.cursor.saturating_add(self.limit).min(total))
    }
}
async fn draft_binding(state: &AppState, id: &str) -> Result<(String, Agent), ApiError> {
    valid_id(id)?;
    let session = state.runs.store.load(id).await.map_err(ApiError::storage)?;
    Ok((
        MemoryStore::scope(&session),
        state.runs.agent_for_session(&session)?,
    ))
}
fn changed() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "skill_draft_changed",
        "Skill draft content or scope changed. Reload and review the current version.",
    )
}

/// Draft provenance must bind every source to the selected scope; never expose another scope's files.
fn visible(draft: &SkillDraftDetails, scope: &str) -> bool {
    let Some(raw) = draft.files.get("references/learning-source.json") else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    value["sources"].as_array().is_some_and(|sources| {
        !sources.is_empty()
            && sources.len() <= 16
            && sources
                .iter()
                .all(|source| source["scope"].as_str() == Some(scope))
    })
}
async fn scoped_draft(
    manager: &kyler_core::skills::SkillManager,
    name: &str,
    scope: &str,
) -> ApiResult<SkillDraftDetails> {
    let draft = manager
        .draft_details(name)
        .await
        .map_err(|_| ApiError::missing())?;
    if !visible(&draft, scope) {
        return Err(ApiError::missing());
    }
    Ok(draft)
}
pub async fn drafts(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(page): Query<DraftPage>,
) -> ApiResult<Json<DraftReviewPage>> {
    let (scope, agent) = draft_binding(&state, &id).await?;
    let manager = agent
        .skill_manager()
        .ok_or_else(|| ApiError::bad("Skills are disabled."))?;
    let summaries = manager
        .drafts()
        .await
        .map_err(|_| ApiError::bad("Unable to inspect skill drafts."))?;
    let mut drafts = Vec::new();
    for summary in summaries {
        if scoped_draft(manager, &summary.name, &scope).await.is_ok() {
            drafts.push(summary);
        }
    }
    let end = page.end(drafts.len())?;
    Ok(Json(DraftReviewPage {
        scope,
        total: drafts.len(),
        next_cursor: (end < drafts.len()).then_some(end),
        drafts: drafts[page.cursor..end].to_vec(),
    }))
}
pub async fn draft(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
) -> ApiResult<Json<DraftReviewDetails>> {
    let (scope, agent) = draft_binding(&state, &id).await?;
    let manager = agent
        .skill_manager()
        .ok_or_else(|| ApiError::bad("Skills are disabled."))?;
    let draft = scoped_draft(manager, &name, &scope).await?;
    let installed_fingerprint = manager
        .list(None, false)
        .await
        .map_err(|_| ApiError::internal())?
        .into_iter()
        .find(|skill| skill.name == name && skill.source == SkillSource::User)
        .map(|skill| skill.fingerprint);
    Ok(Json(DraftReviewDetails {
        scope,
        draft,
        installed_fingerprint,
    }))
}
pub async fn publish(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
    Json(request): Json<PublishDraftRequest>,
) -> ApiResult<Json<SkillInfo>> {
    let (scope, agent) = draft_binding(&state, &id).await?;
    if scope != request.expected_scope {
        return Err(changed());
    }
    let manager = agent
        .skill_manager()
        .ok_or_else(|| ApiError::bad("Skills are disabled."))?;
    let draft = scoped_draft(manager, &name, &scope).await?;
    if draft.info.fingerprint != request.fingerprint {
        return Err(changed());
    }
    manager
        .publish_draft(&name, &request.fingerprint, request.expected_installed)
        .await
        .map(Json)
        .map_err(|_| changed())
}
