use crate::{
    error::{ApiError, valid_id},
    state::AppState,
};

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};

use kyler_config::{McpServerSettings, McpTransport};
use kyler_core::Agent;
use kyler_protocol::api::mcp::{CreateConnectorRequest, McpAction, McpServerInfo};

async fn binding(state: &AppState, id: &str) -> Result<Agent, ApiError> {
    valid_id(id)?;
    let session = state.runs.store.load(id).await.map_err(ApiError::storage)?;
    state.runs.agent_for_session(&session)
}
pub async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<McpServerInfo>>, ApiError> {
    let agent = binding(&state, &id).await?;
    let Some(manager) = agent.mcp_manager() else {
        return Ok(Json(Vec::new()));
    };
    manager
        .list(&id, agent.execution_environment())
        .await
        .map(Json)
        .map_err(|_| ApiError::bad("MCP connections are unavailable for this session."))
}
pub async fn action(
    State(state): State<AppState>,
    Path((id, server, action)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let action = match action.as_str() {
        "connect" => McpAction::Connect,
        "disconnect" => McpAction::Disconnect,
        "reconnect" => McpAction::Reconnect,
        _ => return Err(ApiError::bad("Unknown MCP action.")),
    };
    let agent = binding(&state, &id).await?;
    let manager = agent
        .mcp_manager()
        .ok_or_else(|| ApiError::bad("MCP is not configured."))?;
    manager
        .action(&id, agent.execution_environment(), &server, action)
        .await
        .map_err(|_| ApiError::bad("MCP server is unavailable or not configured."))?;
    Ok(StatusCode::ACCEPTED)
}

pub async fn create(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CreateConnectorRequest>,
) -> Result<(StatusCode, Json<McpServerInfo>), ApiError> {
    let agent = binding(&state, &id).await?;
    let manager = agent
        .mcp_manager()
        .ok_or_else(|| ApiError::bad("MCP is not configured."))?;
    let transport = match request.transport.as_str() {
        "stdio" => McpTransport::Stdio,
        "streamable_http" => McpTransport::StreamableHttp,
        _ => return Err(ApiError::bad("Unsupported connector transport.")),
    };
    if request.args.len() > 64
        || request
            .args
            .iter()
            .any(|arg| arg.len() > 4096 || arg.contains('\0'))
    {
        return Err(ApiError::bad(
            "Connector arguments exceed supported limits.",
        ));
    }
    let settings = McpServerSettings {
        transport,
        url: request.url,
        command: request.command,
        args: request.args,
        bearer_token_env: request.bearer_token_env.filter(|v| !v.is_empty()),
        auto_connect: false,
        ..Default::default()
    };
    let info = manager.add_user_connector(request.id, settings).await
        .map_err(|_| ApiError::bad("Cannot add connector. Use a unique ID, a valid URL or absolute executable path, and configured credential references."))?;
    Ok((StatusCode::CREATED, Json(info)))
}
