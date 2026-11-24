pub mod artifacts;
pub mod attachments;
pub mod automations;
pub mod history;
pub mod inspection;
pub mod mcp;
pub mod memory;
pub mod models;
pub mod permissions;
pub mod plugins;
pub mod preferences;
pub mod processes;
pub mod questions;
pub mod runs;
pub mod sessions;
pub mod skills;
pub mod subagents;
pub mod tasks;
pub mod tools;
pub mod workspaces;

use crate::{error::ApiError, state::AppState};

use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware,
    routing::{get, post},
};
use tower_http::cors::CorsLayer;

use kyler_protocol::api::{
    ServerInfo, attachments::MAX_ATTACHMENT_BYTES, images::MAX_IMAGE_REQUEST_BYTES,
};

pub fn router(state: AppState) -> Router {
    let origins = state
        .runs
        .settings
        .allowed_origins
        .iter()
        .map(|s| s.parse::<HeaderValue>().expect("validated origin"))
        .collect::<Vec<_>>();
    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            axum::http::HeaderName::from_static("last-event-id"),
        ]);
    Router::new()
        .route(
            "/healthz",
            get(|| async { axum::Json(serde_json::json!({"status":"ok"})) }),
        )
        .route("/v1/info", get(info))
        .route("/v1/gateway/identity", get(crate::gateway::identity))
        .route(
            "/v1/gateway/tokens",
            get(crate::gateway::tokens).post(crate::gateway::issue),
        )
        .route(
            "/v1/gateway/tokens/{id}",
            axum::routing::delete(crate::gateway::revoke),
        )
        .route(
            "/v1/gateway/tokens/{id}/rotate",
            post(crate::gateway::rotate),
        )
        .route("/v1/gateway/audit", get(crate::gateway::audit))
        .route("/v1/channels", get(crate::channels::catalog))
        .route("/v1/channels/{provider}", get(crate::channels::status))
        .route("/v1/channels/{provider}/pair", post(crate::channels::pair))
        .route(
            "/v1/channels/{provider}/{chat}",
            axum::routing::delete(crate::channels::remove),
        )
        .route(
            "/v1/preferences",
            get(preferences::get).put(preferences::update),
        )
        .route("/v1/sessions/{id}/active-run", get(sessions::active_run))
        .route(
            "/v1/workspaces/{id}",
            axum::routing::delete(workspaces::delete),
        )
        .route(
            "/v1/workspaces",
            get(workspaces::list).post(workspaces::save),
        )
        .route(
            "/v1/sessions/{session}/processes/{id}/events",
            get(processes::events),
        )
        .route(
            "/v1/sessions/{session}/attachments/{id}/content",
            get(attachments::content),
        )
        .route(
            "/v1/sessions/{id}/memory",
            get(memory::list).put(memory::save),
        )
        .route(
            "/v1/sessions/{session}/memory/{key}",
            axum::routing::delete(memory::delete),
        )
        .route("/v1/sessions/{id}/memory/entries", get(memory::memories))
        .route("/v1/sessions/{id}/memory/entry", get(memory::details))
        .route(
            "/v1/sessions/{id}/memory/review",
            post(memory::review_memory),
        )
        .route("/v1/sessions/{id}/skills/drafts", get(skills::drafts))
        .route(
            "/v1/sessions/{session}/skills/drafts/{name}",
            get(skills::draft),
        )
        .route(
            "/v1/sessions/{session}/skills/drafts/{name}/publish",
            post(skills::publish),
        )
        .route(
            "/v1/sessions/{session}/skills/drafts/{name}/discard",
            post(skills::discard),
        )
        .route("/v1/sessions/{id}/tasks", get(tasks::list))
        .route(
            "/v1/sessions/{session}/tasks/{kind}/{id}",
            get(tasks::get).post(tasks::act),
        )
        .route("/v1/sessions/{id}/questions", get(questions::list))
        .route(
            "/v1/sessions/{session}/questions/{id}/response",
            post(questions::respond),
        )
        .route("/v1/sessions/{id}/history", get(history::list))
        .route("/v1/sessions/{id}/workspace-files", get(workspaces::files))
        .route("/v1/sessions/{id}/artifacts", get(artifacts::list))
        .route(
            "/v1/sessions/{session}/artifacts/{id}/content",
            get(artifacts::download),
        )
        .route("/v1/sessions/{id}/processes", get(processes::list))
        .route(
            "/v1/sessions/{session}/processes/{id}",
            get(processes::get).delete(processes::stop),
        )
        .route(
            "/v1/sessions/{session}/tool-calls/{call}",
            get(tools::details),
        )
        .route(
            "/v1/sessions/{id}/attachments",
            get(attachments::list)
                .post(attachments::upload)
                .layer(DefaultBodyLimit::max(MAX_ATTACHMENT_BYTES)),
        )
        .route(
            "/v1/sessions/{session}/attachments/{id}",
            get(attachments::get).delete(attachments::delete),
        )
        .route(
            "/v1/sessions/{id}/permissions",
            get(permissions::get).put(permissions::select),
        )
        .route("/v1/sessions/{id}/approvals", get(permissions::list))
        .route("/v1/approvals/{id}/decision", post(permissions::decide))
        .route(
            "/v1/sessions/{id}/model/settings",
            get(models::connection).put(models::update_connection),
        )
        .route(
            "/v1/plugins/skills",
            get(plugins::skills).post(plugins::save_skill),
        )
        .route(
            "/v1/plugins/skills/upload",
            post(plugins::upload_skill).layer(DefaultBodyLimit::max(
                kyler_core::skills::MAX_SKILL_ARCHIVE_BYTES,
            )),
        )
        .route("/v1/plugins/skills/reload", post(plugins::reload_skills))
        .route(
            "/v1/plugins/skills/{id}",
            get(plugins::skill_details).delete(plugins::remove_skill),
        )
        .route(
            "/v1/plugins/skills/{id}/{action}",
            post(plugins::skill_action),
        )
        .route(
            "/v1/plugins/connectors",
            get(plugins::connectors).post(plugins::add_connector),
        )
        .route(
            "/v1/plugins/connectors/upload",
            post(plugins::upload_connector).layer(DefaultBodyLimit::max(20 * 1024 * 1024)),
        )
        .route(
            "/v1/plugins/connectors/{id}",
            axum::routing::delete(plugins::remove_connector),
        )
        .route(
            "/v1/plugins/connectors/{id}/{action}",
            post(plugins::connector_action),
        )
        .route(
            "/v1/models/settings",
            get(models::global_connection).put(models::update_global_connection),
        )
        .route(
            "/v1/subagents/settings",
            get(subagents::get).put(subagents::update),
        )
        .route("/v1/providers", get(models::providers))
        .route(
            "/v1/providers/{id}",
            axum::routing::put(models::save_provider).delete(models::delete_provider),
        )
        .route("/v1/providers/{id}/test", post(models::test_provider))
        .route("/v1/models", get(models::list))
        .route("/v1/models/reload", post(models::reload))
        .route(
            "/v1/sessions/{id}/model",
            get(models::get).put(models::select),
        )
        .route(
            "/v1/sessions/{id}/thinking",
            get(models::thinking).put(models::select_thinking),
        )
        .route("/v1/service/shutdown", post(shutdown))
        .route(
            "/v1/sessions/{id}/skills",
            get(skills::list).post(skills::create),
        )
        .route("/v1/sessions/{id}/skills/reload", post(skills::reload))
        .route("/v1/sessions/{id}/skills/{skill}", get(skills::details))
        .route(
            "/v1/sessions/{id}/skills/{skill}/{action}",
            post(skills::action),
        )
        .route("/v1/sessions/{id}/mcp", get(mcp::list).post(mcp::create))
        .route("/v1/sessions/{id}/mcp/{server}/{action}", post(mcp::action))
        .route("/v1/sessions/{id}/usage", get(sessions::usage))
        .route("/v1/sessions", get(sessions::list).post(sessions::create))
        .route(
            "/v1/sessions/{id}",
            get(sessions::get).delete(sessions::delete),
        )
        .route("/v1/sessions/{id}/messages", get(sessions::messages))
        .route(
            "/v1/automations",
            get(automations::list).post(automations::create),
        )
        .route("/v1/automations/status", get(automations::status))
        .route("/v1/automations/preview", post(automations::preview))
        .route(
            "/v1/automations/{id}",
            get(automations::get)
                .put(automations::update)
                .delete(automations::delete),
        )
        .route("/v1/automations/{id}/enabled", post(automations::enabled))
        .route(
            "/v1/automations/{id}/runs",
            get(automations::runs).post(automations::run),
        )
        .route("/v1/automation-runs/{id}/cancel", post(automations::cancel))
        .route("/v1/sessions/{id}/rollout", get(inspection::rollout))
        .route(
            "/v1/sessions/{id}/rollout/{seq}",
            get(inspection::rollout_detail),
        )
        .route(
            "/v1/sessions/{id}/analytics",
            get(inspection::analytics::inspect),
        )
        .route("/v1/sessions/{id}/context", get(inspection::context))
        .route("/v1/sessions/{id}/memory/inspect", get(inspection::memory))
        .route(
            "/v1/sessions/{id}/runs",
            post(runs::create).layer(DefaultBodyLimit::max(MAX_IMAGE_REQUEST_BYTES)),
        )
        .route("/v1/runs/{id}", get(runs::get))
        .route("/v1/runs/{id}/events", get(crate::sse::events))
        .route("/v1/runs/{id}/cancel", post(runs::cancel))
        .fallback(|| async { ApiError::missing() })
        .method_not_allowed_fallback(|| async {
            ApiError::new(
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "This method is not supported.",
            )
        })
        .layer(DefaultBodyLimit::max(state.runs.settings.max_request_bytes))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::gateway::access,
        ))
        .layer(cors)
        .merge(
            Router::new().route(
                "/webhooks/channels/{provider}",
                post(crate::channels::webhook)
                    .get(crate::channels::verify_subscription)
                    .layer(DefaultBodyLimit::max(256 * 1024)),
            ),
        )
        .merge(
            Router::new()
                .route("/channel-api/replies", get(crate::channels::pull_reply))
                .route(
                    "/channel-api/replies/{id}",
                    axum::routing::delete(crate::channels::acknowledge_reply),
                ),
        )
        .with_state(state)
}

async fn info(
    State(state): State<AppState>,
    axum::Extension(principal): axum::Extension<kyler_protocol::api::gateway::Principal>,
) -> axum::Json<ServerInfo> {
    let agent = &state.runs.agent;
    let environment = agent.workspace_environment();
    let model = agent.model_registry().and_then(|r| r.resolve(None).ok());
    axum::Json(ServerInfo {
        subagents_enabled: agent.subagent_manager().is_some(),
        instance_id: Some(state.instance_id.clone()),
        model: model
            .as_ref()
            .map(|m| m.info.model.clone())
            .unwrap_or_else(|| agent.model().into()),
        context_window_tokens: model
            .as_ref()
            .map(|m| m.info.context_window_tokens)
            .unwrap_or_else(|| agent.context_window_tokens()),
        sliding_window_tokens: Some(
            model
                .as_ref()
                .map(|m| m.info.sliding_window_tokens)
                .unwrap_or_else(|| agent.sliding_window_tokens()),
        ),
        default_workspace: if !principal.role.administrative() {
            None
        } else {
            state
                .runs
                .default_workspace
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
        },
        cwd: if principal.role.administrative() {
            environment.map(|e| e.cwd.to_string_lossy().into_owned())
        } else {
            None
        },
    })
}

async fn shutdown(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<StatusCode, ApiError> {
    if headers
        .get("x-kyler-instance-id")
        .and_then(|value| value.to_str().ok())
        != Some(state.instance_id.as_str())
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "instance_mismatch",
            "The service instance changed; shutdown was not requested.",
        ));
    }
    tokio::spawn(async move {
        state.runs.shutdown().await;
    });
    Ok(StatusCode::ACCEPTED)
}
