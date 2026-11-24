//! HTTP adaptation to gateway identities. Unknown non-administrative operations fail closed.
use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use std::collections::HashMap;

use axum::{
    Extension, Json,
    body::Body,
    extract::{FromRequestParts, MatchedPath, Path, Query, RawPathParams, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;

use kyler_protocol::api::gateway::{AuditRecord, CreateToken, Principal, Role, TokenInfo};

pub(crate) fn forbidden() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "access_denied",
        "This identity is not authorized for this operation.",
    )
}
pub(crate) async fn session_allowed(
    state: &AppState,
    principal: &Principal,
    id: &str,
) -> ApiResult<bool> {
    valid_id(id)?;
    let session = state.runs.store.load(id).await.map_err(ApiError::storage)?;
    if !principal.role.administrative() {
        if let Some(environment) = &session.environment {
            let boundary = environment
                .managed
                .as_ref()
                .map(|managed| &managed.root)
                .or_else(|| environment.project_root())
                .unwrap_or(&environment.cwd);
            if !environment.cwd.starts_with(boundary) {
                return Ok(false);
            }
        }
    }
    let workspace = session
        .environment
        .as_ref()
        .map(|e| e.project_root().cloned().unwrap_or_else(|| e.cwd.clone()));
    state
        .runs
        .security
        .session_allowed(principal, id, workspace.as_deref())
        .await
        .map_err(ApiError::storage)
}
async fn authorize(
    state: &AppState,
    principal: &Principal,
    route: &str,
    params: &HashMap<String, String>,
) -> ApiResult<()> {
    let session = if route.starts_with("/v1/sessions/") {
        params
            .get("session")
            .or_else(|| params.get("id"))
            .ok_or_else(forbidden)?
            .clone()
    } else if route.starts_with("/v1/runs/") {
        let id = params.get("id").ok_or_else(forbidden)?;
        state.runs.entry(id).await?.info().session_id
    } else {
        return Ok(());
    };
    if !session_allowed(state, principal, &session).await? {
        return Err(ApiError::missing());
    }
    Ok(())
}
pub async fn access(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let origin_allowed = request.headers().get(header::ORIGIN).is_none_or(|origin| {
        origin.to_str().is_ok_and(|origin| {
            state
                .runs
                .settings
                .allowed_origins
                .iter()
                .any(|allowed| allowed == origin)
        })
    });
    if request.method() == Method::OPTIONS || request.uri().path() == "/healthz" {
        return if origin_allowed {
            next.run(request).await
        } else {
            ApiError::new(
                StatusCode::FORBIDDEN,
                "origin_not_allowed",
                "This browser origin is not permitted.",
            )
            .into_response()
        };
    }
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_string())
        .unwrap_or_else(|| "unmatched".into());
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .split_once(' ')
                .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
                .map(|(_, secret)| secret)
        })
        .filter(|_| {
            request
                .headers()
                .get_all(header::AUTHORIZATION)
                .iter()
                .count()
                == 1
        })
        .unwrap_or_default();
    let principal = match state.runs.security.authenticate(token).await {
        Ok(identity) => identity,
        Err(_) => return ApiError::internal().into_response(),
    };
    let (mut parts, body) = request.into_parts();
    let params = RawPathParams::from_request_parts(&mut parts, &state)
        .await
        .ok()
        .map(|params| {
            params
                .iter()
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let resource = params
        .get("session")
        .or_else(|| params.get("id"))
        .filter(|id| {
            id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .cloned();
    let audit = AuditRecord {
        id: 0,
        request_id: uuid::Uuid::new_v4().to_string(),
        principal_id: principal.as_ref().map(|p| p.id.clone()),
        token_id: principal.as_ref().and_then(|p| p.token_id.clone()),
        action: format!("{} {route}", parts.method),
        resource,
        started_at: chrono::Utc::now().timestamp_millis(),
        finished_at: None,
        status: None,
    };
    let request_id = audit.request_id.clone();
    let audit_id = match state.runs.security.store.begin(audit).await {
        Ok(id) => id,
        Err(_) => return ApiError::internal().into_response(),
    };
    let mut response = if !origin_allowed {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "origin_not_allowed",
            "This browser origin is not permitted.",
        )
        .into_response()
    } else if let Some(principal) = principal {
        match authorize(&state, &principal, &route, &params).await {
            Err(error) => error.into_response(),
            Ok(()) if *state.runs.stopping.borrow() => ApiError::stopping().into_response(),
            Ok(()) => {
                parts.extensions.insert(principal.clone());
                let response = next.run(Request::from_parts(parts, body)).await;
                if response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .is_some_and(|value| value.as_bytes().starts_with(b"text/event-stream"))
                {
                    // Recheck persisted revocation/expiry, including revocation from another process.
                    let (parts, body) = response.into_parts();
                    let mut source = body.into_data_stream();
                    let security = state.runs.security.clone();
                    let stream = async_stream::stream! {
                        loop {
                            if !security.active(&principal).await.unwrap_or(false){break;}
                            tokio::select! {
                                item=source.next()=>match item {
                                    Some(item)=>{if !security.active(&principal).await.unwrap_or(false){break;}yield item;},
                                    None=>break,
                                },
                                _=tokio::time::sleep(std::time::Duration::from_secs(1))=>{},
                            }
                        }
                    };
                    Response::from_parts(parts, Body::from_stream(stream))
                } else {
                    response
                }
            }
        }
    } else {
        let mut response = ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A valid gateway bearer token is required.",
        )
        .into_response();
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        response
    };
    if state
        .runs
        .security
        .store
        .finish(
            audit_id,
            response.status().as_u16(),
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .is_err()
    {
        return ApiError::internal().into_response();
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(&request_id).expect("UUID header"),
    );
    response
}
pub async fn identity(Extension(principal): Extension<Principal>) -> Json<serde_json::Value> {
    Json(
        json!({"principal_id":principal.id,"token_id":principal.token_id,"role":principal.role,"scope":principal.scope}),
    )
}
pub async fn tokens(State(state): State<AppState>) -> ApiResult<Json<Vec<TokenInfo>>> {
    state
        .runs
        .security
        .store
        .list()
        .await
        .map(|tokens| {
            Json(
                tokens
                    .into_iter()
                    .filter(|token| token.role == Role::Owner)
                    .collect(),
            )
        })
        .map_err(ApiError::storage)
}
pub async fn issue(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(request): Json<CreateToken>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    if request.role != Role::Owner {
        return Err(ApiError::bad("Only owner tokens can be created."));
    }
    let token = state
        .runs
        .security
        .issue(&principal, request)
        .await
        .map_err(|_| ApiError::bad("Invalid token name, expiry or scope."))?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"token":token.secret,"info":token.info})),
    ))
}
pub async fn revoke(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if state
        .runs
        .security
        .revoke(&principal, &id)
        .await
        .map_err(ApiError::storage)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::missing())
    }
}
pub async fn rotate(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let token = state
        .runs
        .security
        .rotate(&principal, &id)
        .await
        .map_err(|_| {
            ApiError::bad(
                "Token cannot be rotated; check its ID, owner role, revocation and expiry.",
            )
        })?;
    Ok(Json(json!({"token":token.secret,"info":token.info})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditQuery {
    pub before: Option<i64>,
    pub limit: Option<usize>,
}
pub async fn audit(
    State(state): State<AppState>,
    Query(query): Query<AuditQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let limit = query.limit.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err(ApiError::bad("limit must be in 1..=500"));
    }
    let records = state
        .runs
        .security
        .store
        .audit(query.before, limit)
        .await
        .map_err(ApiError::storage)?;
    let next = records.last().map(|record| record.id);
    Ok(Json(json!({"records":records,"next_before":next})))
}
