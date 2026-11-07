//! Session inspection reads committed checkpoints; it never runs tools or models.
use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use kyler_core::{
    context::{ContentItem, ToolResultStatus},
    memory::{MemoryEntry, MemoryStore},
    rollout::{RolloutPayload, RolloutRecord},
};
use kyler_protocol::api::inspection::{
    ContextBudget, ContextInspection, RolloutDetail, RolloutItem, RolloutPage,
};

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Pagination {
    cursor: u64,
    limit: Option<usize>,
}

fn item(record: &RolloutRecord) -> RolloutItem {
    let (kind, title, status) = match &record.payload {
        RolloutPayload::SessionCreated { .. } => ("session", "Session created".into(), None),
        RolloutPayload::EventRecorded { event } => {
            let tools = event
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentItem::ToolCall { name, .. } | ContentItem::ToolResult { name, .. } => {
                        Some(name.as_str())
                    }
                    _ => None,
                })
                .take(3)
                .collect::<Vec<_>>();
            if !tools.is_empty() {
                let status = if event.content.iter().any(|part| {
                    matches!(
                        part,
                        ContentItem::ToolResult {
                            status: ToolResultStatus::Error,
                            ..
                        }
                    )
                }) {
                    "failed"
                } else if event
                    .content
                    .iter()
                    .any(|part| matches!(part, ContentItem::ToolResult { .. }))
                {
                    "completed"
                } else {
                    "started"
                };
                ("tool", tools.join(", "), Some(status.into()))
            } else {
                ("message", format!("{} message", event.author.name), None)
            }
        }
        RolloutPayload::ExecutionStarted { execution } => (
            "execution",
            format!("Execution started · step {}", execution.current_step),
            Some("running".into()),
        ),
        RolloutPayload::ExecutionUpdated { execution }
        | RolloutPayload::ExecutionFinished { execution }
        | RolloutPayload::ExecutionArchived { execution } => (
            "execution",
            format!("Execution · step {}", execution.current_step),
            Some(format!("{:?}", execution.status).to_ascii_lowercase()),
        ),
        RolloutPayload::ContextCompacted { .. } => ("context", "Context compressed".into(), None),
        RolloutPayload::ContextUpdated { .. } => ("context", "Context checkpoint".into(), None),
        RolloutPayload::UsageUpdated { .. } => ("usage", "Token usage updated".into(), None),
        RolloutPayload::StateUpdated { .. } => ("state", "Application state updated".into(), None),
        RolloutPayload::EnvironmentUpdated { .. } => {
            ("environment", "Environment updated".into(), None)
        }
        RolloutPayload::SubagentUpdated { .. } | RolloutPayload::SubagentRemoved { .. } => {
            ("subagent", "Subagent updated".into(), None)
        }
        RolloutPayload::CheckpointCommitted { revision, .. } => (
            "checkpoint",
            format!("Checkpoint {revision}"),
            Some("committed".into()),
        ),
    };
    RolloutItem {
        seq: record.seq,
        timestamp_ms: record.timestamp_ms,
        execution_id: record.execution_id.clone(),
        kind: kind.into(),
        title,
        status,
    }
}
fn detail(record: RolloutRecord) -> ApiResult<RolloutDetail> {
    let item = item(&record);
    let mut data = match &record.payload {
        RolloutPayload::SessionCreated { session } => {
            json!({"session_id":session.session_id,"created_at":session.created_at,"revision":session.revision})
        }
        RolloutPayload::StateUpdated { set, removed } => {
            json!({"updated_keys":set.keys().collect::<Vec<_>>(),"removed_keys":removed})
        }
        _ => serde_json::to_value(&record.payload).map_err(|_| ApiError::internal())?,
    };
    super::tools::redact(&mut data);
    fn bound(value: &mut Value) {
        match value {
            Value::String(text) if text.len() > 64000 => {
                *text = format!(
                    "{}\n[Preview truncated]",
                    crate::runs::events::prefix(text, 64000)
                )
            }
            Value::Array(items) => items.iter_mut().for_each(bound),
            Value::Object(items) => items.values_mut().for_each(bound),
            _ => {}
        }
    }
    bound(&mut data);
    Ok(RolloutDetail { item, data })
}
pub async fn rollout(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<Pagination>,
) -> ApiResult<Json<RolloutPage>> {
    valid_id(&id)?;
    let limit = query.limit.unwrap_or(60);
    if !(1..=200).contains(&limit) {
        return Err(ApiError::bad("limit must be in 1..=200."));
    }
    let mut reader = state
        .runs
        .store
        .rollout(&id)
        .await
        .map_err(ApiError::storage)?;
    let mut items = Vec::new();
    let mut cursor = query.cursor;
    let mut more = false;
    while let Some(record) = reader.next().await.map_err(ApiError::storage)? {
        if record.seq <= query.cursor {
            continue;
        }
        if items.len() == limit {
            more = true;
            break;
        }
        cursor = record.seq;
        items.push(item(&record));
    }
    if !more && query.cursor > reader.position().last_seq {
        return Err(ApiError::bad(
            "Rollout cursor is beyond the committed history.",
        ));
    }
    Ok(Json(RolloutPage {
        session_id: id,
        items,
        cursor,
        next_cursor: more.then_some(cursor),
    }))
}
pub async fn rollout_detail(
    State(state): State<AppState>,
    Path((id, seq)): Path<(String, u64)>,
) -> ApiResult<Json<RolloutDetail>> {
    valid_id(&id)?;
    let mut reader = state
        .runs
        .store
        .rollout(&id)
        .await
        .map_err(ApiError::storage)?;
    while let Some(record) = reader.next().await.map_err(ApiError::storage)? {
        if record.seq == seq {
            return detail(record).map(Json);
        }
        if record.seq > seq {
            break;
        }
    }
    Err(ApiError::missing())
}
pub async fn context(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ContextInspection>> {
    valid_id(&id)?;
    let session = state
        .runs
        .store
        .load(&id)
        .await
        .map_err(ApiError::storage)?;
    let selection = state.runs.read_model_selection(&id).await?;
    let (budget, budget_error) = match state.runs.agent.bind_model(selection.profile.as_deref()) {
        Ok(agent) => {
            let policy = agent.context_policy();
            (
                Some(ContextBudget {
                    model: agent.model().into(),
                    context_tokens: policy.context_tokens,
                    sliding_window_tokens: policy.sliding_window_tokens(),
                    input_limit: policy.input_limit(),
                    output_tokens: policy.output_tokens,
                    safety_tokens: policy.safety_tokens,
                }),
                None,
            )
        }
        Err(_) => (
            None,
            Some(
                "The selected model is unavailable. The last request snapshot is retained.".into(),
            ),
        ),
    };
    Ok(Json(ContextInspection {
        session_id: id,
        latest_request: session.memory.last_request.clone(),
        next_request_budget: budget,
        budget_error,
        compression_count: session.memory.compression_count,
        last_compaction: session.memory.last_compaction.clone(),
        window_start: session.memory.window_start,
        total_events: session.events.len(),
        cumulative_input_tokens: u64::from(session.usage.prompt_tokens)
            + u64::from(session.subagent_usage.prompt_tokens),
        cumulative_output_tokens: u64::from(session.usage.completion_tokens)
            + u64::from(session.subagent_usage.completion_tokens),
    }))
}
#[derive(Serialize)]
pub struct MemoryInspection {
    session_id: String,
    workspace_root: Option<String>,
    scope: String,
    summary: Option<String>,
    summary_timestamp_ms: Option<i64>,
    compression_count: usize,
    summarized_events: usize,
    retrieved_keys: Vec<String>,
    entries: Vec<MemoryEntry>,
    activity: Vec<Value>,
}
pub async fn memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<MemoryInspection>> {
    valid_id(&id)?;
    let session = state
        .runs
        .store
        .load(&id)
        .await
        .map_err(ApiError::storage)?;
    let scope = MemoryStore::scope(&session);
    let store = state.runs.memories.clone();
    let store_scope = scope.clone();
    let entries = tokio::task::spawn_blocking(move || store.list(&store_scope))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(ApiError::storage)?;
    let results = session
        .events
        .iter()
        .flat_map(|event| {
            event.content.iter().filter_map(move |part| match part {
                ContentItem::ToolResult {
                    tool_call_id,
                    status,
                    ..
                } => Some((
                    (event.execution_id.as_str(), tool_call_id.as_str()),
                    *status,
                )),
                _ => None,
            })
        })
        .collect::<HashMap<_, _>>();
    let mut activity = Vec::new();
    'events: for event in session.events.iter().rev() {
        for part in &event.content {
            if let ContentItem::ToolCall {
                name,
                arguments,
                tool_call_id,
            } = part
            {
                if !matches!(name.as_str(), "memory" | "project_memory") {
                    continue;
                }
                let status =
                    match results.get(&(event.execution_id.as_str(), tool_call_id.as_str())) {
                        Some(ToolResultStatus::Success) => "success",
                        Some(ToolResultStatus::Error) => "error",
                        None => "pending",
                    };
                activity.push(json!({"timestamp":event.timestamp,"call_id":tool_call_id,"action":arguments.get("action").and_then(Value::as_str),"key":arguments.get("key").and_then(Value::as_str),"execution_id":event.execution_id,"status":status}));
                if activity.len() == 30 {
                    break 'events;
                }
            }
        }
    }
    Ok(Json(MemoryInspection {
        session_id: id,
        workspace_root: session
            .environment
            .as_ref()
            .map(|e| e.workspace_root.to_string_lossy().into_owned()),
        scope,
        summary: session.memory.summary.clone(),
        summary_timestamp_ms: session
            .memory
            .last_compaction
            .as_ref()
            .map(|snapshot| snapshot.timestamp_ms),
        compression_count: session.memory.compression_count,
        summarized_events: session.memory.window_start,
        retrieved_keys: session
            .memory
            .last_request
            .as_ref()
            .map(|r| r.retrieved_memory_keys.clone())
            .unwrap_or_default(),
        entries,
        activity,
    }))
}
