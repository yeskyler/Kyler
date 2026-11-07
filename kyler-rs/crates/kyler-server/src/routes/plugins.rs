//! User plugin configuration is available before a workspace or session exists.
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
};

use kyler_config::{McpServerSettings, McpTransport};
use kyler_protocol::api::{
    mcp::{CreateConnectorRequest, McpAction, McpServerInfo},
    skills::{
        SaveSkillRequest, SkillAction, SkillActionRequest, SkillDetails, SkillInfo, SkillScope,
    },
};

pub async fn skills(State(state): State<AppState>) -> ApiResult<Json<Vec<SkillInfo>>> {
    let Some(manager) = state.runs.agent.skill_manager() else {
        return Ok(Json(Vec::new()));
    };
    manager
        .list(None, false)
        .await
        .map(Json)
        .map_err(|_| ApiError::bad("Unable to discover user and system skills."))
}
pub async fn upload_skill(
    State(state): State<AppState>,
    bytes: Bytes,
) -> ApiResult<(StatusCode, Json<SkillInfo>)> {
    let manager = state
        .runs
        .agent
        .skill_manager()
        .ok_or_else(|| ApiError::bad("Skills are disabled."))?;
    manager.install_archive(bytes.to_vec()).await
        .map(|info| (StatusCode::CREATED, Json(info)))
        .map_err(|error| {
            if error.to_string().contains("already exists") {
                ApiError::new(StatusCode::CONFLICT, "skill_exists", "A user skill with this name already exists. Remove it before installing again.")
            } else {
                ApiError::bad("Invalid skill ZIP. Include exactly one SKILL.md at the root or in one folder; links, unsafe paths and oversized files are rejected (8 MiB expanded maximum).")
            }
        })
}
pub async fn remove_skill(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SkillActionRequest>,
) -> ApiResult<StatusCode> {
    let manager = state
        .runs
        .agent
        .skill_manager()
        .ok_or_else(|| ApiError::bad("Skills are disabled."))?;
    manager.remove_user(&id, &request.fingerprint).await
        .map_err(|_| ApiError::bad("Cannot remove skill. Only unchanged user skills can be removed; reload and inspect the current version."))?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn reload_skills(State(state): State<AppState>) -> ApiResult<Json<Vec<SkillInfo>>> {
    let manager = state
        .runs
        .agent
        .skill_manager()
        .ok_or_else(|| ApiError::bad("Skills are disabled."))?;
    manager
        .list(None, true)
        .await
        .map(Json)
        .map_err(|_| ApiError::bad("Unable to reload user and system skills."))
}
pub async fn save_skill(
    State(state): State<AppState>,
    Json(request): Json<SaveSkillRequest>,
) -> ApiResult<(StatusCode, Json<SkillInfo>)> {
    if request.scope != SkillScope::User || request.expected_fingerprint.is_some() {
        return Err(ApiError::bad("Only new user skills can be added here."));
    }
    let manager = state
        .runs
        .agent
        .skill_manager()
        .ok_or_else(|| ApiError::bad("Skills are disabled."))?;
    manager
        .save(None, request)
        .await
        .map(|info| (StatusCode::CREATED, Json(info)))
        .map_err(|_| {
            ApiError::bad(
                "Cannot save skill. Check its name, metadata, and whether it already exists.",
            )
        })
}
pub async fn skill_details(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SkillDetails>> {
    let manager = state
        .runs
        .agent
        .skill_manager()
        .ok_or_else(|| ApiError::bad("Skills are disabled."))?;
    manager
        .details(None, &id)
        .await
        .map(Json)
        .map_err(|_| ApiError::bad("Skill unavailable or changed. Reload before reviewing."))
}
pub async fn skill_action(
    State(state): State<AppState>,
    Path((id, action)): Path<(String, String)>,
    Json(request): Json<SkillActionRequest>,
) -> ApiResult<StatusCode> {
    let action = match action.as_str() {
        "enable" => SkillAction::Enable,
        "disable" => SkillAction::Disable,
        _ => return Err(ApiError::bad("Unknown skill action.")),
    };
    let manager = state
        .runs
        .agent
        .skill_manager()
        .ok_or_else(|| ApiError::bad("Skills are disabled."))?;
    manager
        .action(None, &id, action, &request.fingerprint)
        .await
        .map_err(|_| ApiError::bad("Skill changed. Reload and review its current version."))?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn connectors(State(state): State<AppState>) -> ApiResult<Json<Vec<McpServerInfo>>> {
    let Some(manager) = state.runs.agent.mcp_manager() else {
        return Ok(Json(Vec::new()));
    };
    manager
        .system_list()
        .await
        .map(Json)
        .map_err(|_| ApiError::bad("Connector configuration is unavailable."))
}
pub async fn add_connector(
    State(state): State<AppState>,
    Json(request): Json<CreateConnectorRequest>,
) -> ApiResult<(StatusCode, Json<McpServerInfo>)> {
    let transport = match request.transport.as_str() {
        "stdio" => McpTransport::Stdio,
        "streamable_http" => McpTransport::StreamableHttp,
        _ => return Err(ApiError::bad("Unsupported connector transport.")),
    };
    if request.args.len() > 64
        || request
            .args
            .iter()
            .any(|value| value.len() > 4096 || value.contains('\0'))
    {
        return Err(ApiError::bad(
            "Connector arguments exceed supported limits.",
        ));
    }
    let manager = state
        .runs
        .agent
        .mcp_manager()
        .ok_or_else(|| ApiError::bad("MCP is not configured."))?;
    let settings = McpServerSettings {
        transport,
        url: request.url,
        command: request.command,
        args: request.args,
        bearer_token_env: request.bearer_token_env.filter(|value| !value.is_empty()),
        auto_connect: false,
        ..Default::default()
    };
    let info = manager.add_user_connector(request.id, settings).await.map_err(|_| ApiError::bad("Cannot add connector. Check its ID, endpoint, executable path and credential references."))?;
    Ok((StatusCode::CREATED, Json(info)))
}

pub async fn remove_connector(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let manager = state
        .runs
        .agent
        .mcp_manager()
        .ok_or_else(|| ApiError::bad("MCP is not configured."))?;
    manager.remove_user_connector(&id).await.map_err(|_| ApiError::bad("Cannot uninstall connector. Only user-added connectors can be removed; refresh the list and try again."))?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn connector_action(
    State(state): State<AppState>,
    Path((id, action)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let action = match action.as_str() {
        "connect" => McpAction::Connect,
        "disconnect" => McpAction::Disconnect,
        "reconnect" => McpAction::Reconnect,
        _ => return Err(ApiError::bad("Unknown connector action.")),
    };
    let manager = state
        .runs
        .agent
        .mcp_manager()
        .ok_or_else(|| ApiError::bad("MCP is not configured."))?;
    manager
        .system_action(&id, action)
        .await
        .map_err(|_| ApiError::bad("Connector unavailable. Check its system configuration."))?;
    Ok(StatusCode::ACCEPTED)
}

pub async fn upload_connector(
    State(state): State<AppState>,
    bytes: Bytes,
) -> ApiResult<(StatusCode, Json<McpServerInfo>)> {
    let manager = state
        .runs
        .agent
        .mcp_manager()
        .ok_or_else(|| ApiError::bad("MCP is not configured."))?;
    let info = manager
        .install_package(bytes.to_vec())
        .await
        .map_err(|error| ApiError::bad(&format!("Cannot install connector package: {error}")))?;
    Ok((StatusCode::CREATED, Json(info)))
}
