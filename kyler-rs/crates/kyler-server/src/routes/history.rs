//! Structured projection of durable events, independent of SSE replay retention.
use super::sessions::{Pagination, page};
use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Path, Query, State, rejection::QueryRejection},
};

use kyler_core::context::ContentItem;
use kyler_protocol::api::history::{HistoryContent, HistoryEvent, HistoryExecution, HistoryPage};

pub async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
    query: Result<Query<Pagination>, QueryRejection>,
) -> ApiResult<Json<HistoryPage>> {
    valid_id(&id)?;
    let (cursor, limit) = page(query)?;
    let session = state
        .runs
        .store
        .load(&id)
        .await
        .map_err(ApiError::storage)?;
    if cursor > session.events.len() {
        return Err(ApiError::bad("Pagination cursor is out of range."));
    }
    let requested_end = cursor.saturating_add(limit).min(session.events.len());
    let mut end = cursor;
    let mut bytes = 0usize;
    let executions = session
        .executions
        .iter()
        .chain(session.execution.iter())
        .map(|execution| (execution.execution_id.as_str(), execution))
        .collect::<HashMap<_, _>>();
    let mut included = HashSet::new();
    let mut events = Vec::new();
    for event in &session.events[cursor..requested_end] {
        let execution = executions.get(event.execution_id.as_str());
        let mut content = Vec::new();
        for item in &event.content {
            content.push(match item {
                ContentItem::ContextCompaction { compaction } => HistoryContent::ContextCompaction { compaction: compaction.clone() },
                ContentItem::Reasoning { text } => HistoryContent::Reasoning { text:text.clone() },
                ContentItem::Message { role, content } => HistoryContent::Message { role: role.as_str().into(), text: content.clone() },
                ContentItem::Attachment { attachment } => HistoryContent::Attachment { attachment: attachment.clone() },
                ContentItem::Image { image } => HistoryContent::Image { name: image.name().into() },
                ContentItem::ToolCall { tool_call_id, name, arguments } => {
                    let presentation = execution.and_then(|execution| execution.tool_presentations.get(tool_call_id)).cloned();
                    let mut arguments = arguments.clone();
                    super::tools::redact(&mut arguments);
                    let summary = presentation.as_ref().and_then(|metadata| crate::runs::execution::tool_summary(name, &arguments, &metadata.source));
                    if arguments.to_string().len() > 8192 { arguments = serde_json::json!({"preview":crate::runs::events::prefix(&arguments.to_string(),8192),"truncated":true}); }
                    HistoryContent::ToolCall { call_id: tool_call_id.clone(), name: name.clone(), arguments, presentation, summary }
                }
                ContentItem::ToolResult { tool_call_id, name, status, content, .. } => HistoryContent::ToolResult { call_id: tool_call_id.clone(), name: name.clone(), status: format!("{status:?}").to_ascii_lowercase(), output: crate::runs::events::prefix(content, 32000), output_bytes: content.len(), truncated: content.len() > 32000 },
            });
        }
        let projected = HistoryEvent {
            id: event.id.clone(),
            execution_id: event.execution_id.clone(),
            timestamp_ms: event.timestamp.saturating_mul(1000),
            content,
        };
        let size = serde_json::to_vec(&projected)
            .map_err(|_| ApiError::internal())?
            .len();
        if !events.is_empty() && bytes.saturating_add(size) > 2 * 1024 * 1024 {
            break;
        }
        bytes = bytes.saturating_add(size);
        included.insert(event.execution_id.as_str());
        events.push(projected);
        end += 1;
    }
    let executions = session
        .executions
        .iter()
        .chain(session.execution.iter())
        .filter(|execution| included.contains(execution.execution_id.as_str()))
        .map(|execution| HistoryExecution {
            execution_id: execution.execution_id.clone(),
            status: format!("{:?}", execution.status).to_ascii_lowercase(),
            started_at: execution.started_at,
            finished_at: execution.finished_at,
        })
        .collect();
    Ok(Json(HistoryPage {
        events,
        executions,
        next_cursor: (end < session.events.len()).then_some(end),
    }))
}
