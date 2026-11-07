pub mod artifacts;
pub mod attachments;
pub mod history;
pub mod inspection;
pub mod mcp;
pub mod memory;
pub mod models;
pub mod permissions;
pub mod plugins;
pub mod processes;
pub mod questions;
pub mod runs;
pub mod sessions;
pub mod skills;
pub mod tasks;
pub mod tools;
pub mod workspaces;

use crate::{error::ApiError, state::AppState};

use axum::{
    Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
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
        .route("/v1/models", get(models::list))
        .route("/v1/models/reload", post(models::reload))
        .route(
            "/v1/sessions/{id}/model",
            get(models::get).put(models::select),
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
        .route("/v1/sessions/{id}/rollout", get(inspection::rollout))
        .route(
            "/v1/sessions/{id}/rollout/{seq}",
            get(inspection::rollout_detail),
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
        .layer(middleware::from_fn_with_state(state.clone(), access))
        .layer(cors)
        .with_state(state)
}
async fn access(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let settings = &state.runs.settings;
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        if !origin.to_str().is_ok_and(|origin| {
            settings
                .allowed_origins
                .iter()
                .any(|allowed| allowed == origin)
        }) {
            return ApiError::new(
                StatusCode::FORBIDDEN,
                "origin_not_allowed",
                "This browser origin is not permitted.",
            )
            .into_response();
        }
    }
    if request.method() != Method::OPTIONS
        && request.uri().path() != "/healthz"
        && !settings.access_token.is_empty()
    {
        let expected = format!("Bearer {}", settings.access_token.expose());
        let supplied = request
            .headers()
            .get(header::AUTHORIZATION)
            .map(|v| v.as_bytes())
            .unwrap_or_default();
        if !equal(supplied, expected.as_bytes()) {
            let mut response = ApiError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "A valid bearer token is required.",
            )
            .into_response();
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            return response;
        }
    }
    if *state.runs.stopping.borrow() && request.uri().path() != "/healthz" {
        return ApiError::stopping().into_response();
    }
    next.run(request).await
}
fn equal(a: &[u8], b: &[u8]) -> bool {
    let mut difference = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        difference |= (a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0)) as usize;
    }
    difference == 0
}

async fn info(State(state): State<AppState>) -> axum::Json<ServerInfo> {
    let agent = &state.runs.agent;
    let environment = agent.workspace_environment();
    let model = agent.model_registry().and_then(|r| r.resolve(None).ok());
    axum::Json(ServerInfo {
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
        default_workspace: state
            .runs
            .default_workspace
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        workspace_root: environment.map(|e| e.workspace_root.to_string_lossy().into_owned()),
        working_directory: environment.map(|e| e.working_directory.to_string_lossy().into_owned()),
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
