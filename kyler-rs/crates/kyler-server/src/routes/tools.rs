use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use axum::{
    Json,
    extract::{Path, State},
};

use kyler_core::context::ContentItem;
use kyler_protocol::api::tools::ToolDetails;

pub async fn details(
    State(state): State<AppState>,
    Path((session, call)): Path<(String, String)>,
) -> ApiResult<Json<ToolDetails>> {
    valid_id(&session)?;
    if call.is_empty() || call.len() > 512 {
        return Err(ApiError::bad("Invalid tool call ID"));
    }
    let snapshot = state
        .runs
        .store
        .load(&session)
        .await
        .map_err(ApiError::storage)?;
    let found = snapshot
        .events
        .iter()
        .rev()
        .find_map(|event| {
            event.content.iter().find_map(|item| match item {
                ContentItem::ToolCall {
                    tool_call_id,
                    name,
                    arguments,
                } if tool_call_id == &call => {
                    Some((event.execution_id.clone(), name.clone(), arguments.clone()))
                }
                _ => None,
            })
        })
        .ok_or_else(ApiError::missing)?;
    let result = snapshot
        .events
        .iter()
        .rev()
        .filter(|event| event.execution_id == found.0)
        .flat_map(|event| &event.content)
        .find_map(|item| match item {
            ContentItem::ToolResult {
                tool_call_id,
                status,
                content,
                ..
            } if tool_call_id == &call => {
                Some((format!("{status:?}").to_ascii_lowercase(), content.clone()))
            }
            _ => None,
        });
    let (status, output) = result.unwrap_or_else(|| ("pending".into(), String::new()));
    let bytes = output.len();
    let mut arguments = found.2;
    redact(&mut arguments);
    if arguments.to_string().len() > 8192 {
        arguments = serde_json::json!({"preview":super::super::runs::events::prefix(&arguments.to_string(),8192),"truncated":true});
    }
    Ok(Json(ToolDetails {
        call_id: call,
        execution_id: found.0,
        name: found.1,
        arguments,
        status,
        output: super::super::runs::events::prefix(&output, 32000),
        output_bytes: bytes,
        truncated: bytes > 32000,
    }))
}
pub(super) fn redact(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                let key = key.to_ascii_lowercase();
                if [
                    "api_key",
                    "password",
                    "authorization",
                    "secret",
                    "access_token",
                ]
                .iter()
                .any(|s| key.contains(s))
                {
                    *value = serde_json::Value::String("[REDACTED]".into());
                } else {
                    redact(value);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact(item);
            }
        }
        _ => {}
    }
}
