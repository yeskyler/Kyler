//! Read committed metadata for charts; never return arguments, messages or tool output.
use crate::{
    error::{ApiError, ApiResult, valid_id},
    state::AppState,
};

use std::collections::{BTreeMap, HashMap, VecDeque};

use axum::{
    Json,
    extract::{Path, State},
};

use kyler_core::{
    context::{ContentItem, ContextMemory, Event, ExecutionContext, ToolResultStatus},
    rollout::{RolloutPayload, RolloutRecord},
};
use kyler_protocol::api::inspection::{
    ActivityBucket, CompactionSnapshot, ContextTrendPoint, ExecutionAggregate, InspectionAnalytics,
    ToolAggregate,
};

#[derive(Default)]
struct Aggregate {
    records: u64,
    last_activity_ms: Option<i64>,
    events: HashMap<String, ReducedEvent>,
    executions: HashMap<String, ExecutionAggregate>,
    requests: BTreeMap<(i64, String, u32), ContextTrendPoint>,
    compactions: BTreeMap<i64, CompactionSnapshot>,
    subagents: Vec<i64>,
}
struct ReducedEvent {
    execution: String,
    timestamp: i64,
    seq: u128,
    messages: u64,
    tools: Vec<ReducedTool>,
}
enum ReducedTool {
    Call { id: String, name: String },
    Result { id: String, failed: bool },
}
impl Aggregate {
    fn add(&mut self, record: RolloutRecord) {
        self.records += 1;
        self.last_activity_ms = Some(
            self.last_activity_ms
                .map_or(record.timestamp_ms, |previous| {
                    previous.max(record.timestamp_ms)
                }),
        );
        match record.payload {
            RolloutPayload::SessionCreated { session } => {
                for (index, event) in session.events.into_iter().enumerate() {
                    self.add_event(event, ((record.seq as u128) << 64) + index as u128);
                }
                for execution in session.executions.into_iter().chain(session.execution) {
                    self.add_execution(execution);
                }
                self.add_memory(session.memory);
            }
            RolloutPayload::EventRecorded { event } => {
                self.add_event(event, (record.seq as u128) << 64)
            }
            RolloutPayload::ExecutionStarted { execution }
            | RolloutPayload::ExecutionUpdated { execution }
            | RolloutPayload::ExecutionFinished { execution }
            | RolloutPayload::ExecutionArchived { execution } => self.add_execution(execution),
            RolloutPayload::ContextUpdated { memory }
            | RolloutPayload::ContextCompacted { memory } => self.add_memory(memory),
            RolloutPayload::SubagentUpdated { .. } | RolloutPayload::SubagentRemoved { .. } => {
                self.subagents.push(record.timestamp_ms)
            }
            _ => {}
        }
    }
    fn add_event(&mut self, event: Event, seq: u128) {
        let mut reduced = ReducedEvent {
            execution: event.execution_id,
            timestamp: event.timestamp.saturating_mul(1000),
            seq: self
                .events
                .get(&event.id)
                .map_or(seq, |previous| previous.seq),
            messages: 0,
            tools: Vec::new(),
        };
        for part in event.content {
            match part {
                ContentItem::Message { .. } => reduced.messages += 1,
                ContentItem::ToolCall {
                    name, tool_call_id, ..
                } => reduced.tools.push(ReducedTool::Call {
                    id: tool_call_id,
                    name,
                }),
                ContentItem::ToolResult {
                    tool_call_id,
                    status,
                    ..
                } => reduced.tools.push(ReducedTool::Result {
                    id: tool_call_id,
                    failed: status == ToolResultStatus::Error,
                }),
                _ => {}
            }
        }
        // Streaming updates can replace an already committed event.
        self.events.insert(event.id, reduced);
    }
    fn add_execution(&mut self, execution: ExecutionContext) {
        self.executions.insert(
            execution.execution_id.clone(),
            ExecutionAggregate {
                execution_id: execution.execution_id,
                status: format!("{:?}", execution.status).to_ascii_lowercase(),
                started_at: execution.started_at,
                finished_at: execution.finished_at,
                steps: execution.current_step,
            },
        );
    }
    fn add_memory(&mut self, memory: ContextMemory) {
        if let Some(request) = memory.last_request {
            let point = ContextTrendPoint {
                execution_id: request.execution_id.clone(),
                step: request.step,
                timestamp_ms: request.timestamp_ms,
                model: request.budget.model,
                capacity: request.budget.context_tokens,
                input_tokens: request
                    .provider_input_tokens
                    .unwrap_or(request.estimated_input_tokens),
                estimated: request.provider_input_tokens.is_none(),
            };
            self.requests.insert(
                (request.timestamp_ms, request.execution_id, request.step),
                point,
            );
        }
        if let Some(snapshot) = memory.last_compaction {
            self.compactions.insert(snapshot.timestamp_ms, snapshot);
        }
    }
    fn finish(self, session_id: String) -> InspectionAnalytics {
        let mut events = self.events.into_values().collect::<Vec<_>>();
        events.sort_by_key(|event| event.seq);
        let mut tools = BTreeMap::<String, ToolAggregate>::new();
        let mut pending = HashMap::<(String, String), VecDeque<String>>::new();
        let mut activity = BTreeMap::<i64, ActivityBucket>::new();
        for event in events {
            if event.messages > 0 {
                activity.entry(event.timestamp).or_default().messages += event.messages;
            }
            for part in event.tools {
                match part {
                    ReducedTool::Call { id, name } => {
                        let aggregate =
                            tools.entry(name.clone()).or_insert_with(|| ToolAggregate {
                                name: name.clone(),
                                ..Default::default()
                            });
                        aggregate.calls += 1;
                        aggregate.pending += 1;
                        pending
                            .entry((event.execution.clone(), id))
                            .or_default()
                            .push_back(name);
                        activity.entry(event.timestamp).or_default().tools += 1;
                    }
                    ReducedTool::Result { id, failed } => {
                        if let Some(name) = pending
                            .get_mut(&(event.execution.clone(), id))
                            .and_then(VecDeque::pop_front)
                        {
                            let aggregate = tools.get_mut(&name).expect("call aggregate exists");
                            aggregate.pending -= 1;
                            if failed {
                                aggregate.failed += 1;
                            } else {
                                aggregate.succeeded += 1;
                            }
                        }
                    }
                }
            }
        }
        for timestamp in self.subagents {
            activity.entry(timestamp).or_default().subagents += 1;
        }
        for timestamp in self.compactions.keys() {
            activity.entry(*timestamp).or_default().compactions += 1;
        }
        let mut tools = tools.into_values().collect::<Vec<_>>();
        let tool_calls = tools.iter().map(|tool| tool.calls).sum();
        let failed_tool_calls = tools.iter().map(|tool| tool.failed).sum();
        tools.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.name.cmp(&b.name)));
        if tools.len() > 20 {
            let mut other = ToolAggregate {
                name: "Other tools".into(),
                ..Default::default()
            };
            for tool in tools.drain(20..) {
                other.calls += tool.calls;
                other.succeeded += tool.succeeded;
                other.failed += tool.failed;
                other.pending += tool.pending;
            }
            tools.push(other);
        }
        let mut buckets = Vec::<ActivityBucket>::new();
        if let (Some(first), Some(last)) = (activity.first_key_value(), activity.last_key_value()) {
            let start = *first.0;
            let width = ((*last.0 - start).max(0) / 24 + 1).max(1000);
            let mut grouped = BTreeMap::<i64, ActivityBucket>::new();
            for (timestamp, value) in activity {
                let timestamp_ms = start + (timestamp - start) / width * width;
                let bucket = grouped
                    .entry(timestamp_ms)
                    .or_insert_with(|| ActivityBucket {
                        timestamp_ms,
                        ..Default::default()
                    });
                bucket.messages += value.messages;
                bucket.tools += value.tools;
                bucket.compactions += value.compactions;
                bucket.subagents += value.subagents;
            }
            buckets.extend(grouped.into_values());
        }
        let context_request_count = self.requests.len();
        let context_requests = self
            .requests
            .into_values()
            .rev()
            .take(300)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let compactions = self
            .compactions
            .into_values()
            .rev()
            .take(100)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let execution_count = self.executions.len();
        let mut executions = self.executions.into_values().collect::<Vec<_>>();
        executions.sort_by_key(|execution| execution.started_at);
        if executions.len() > 100 {
            executions.drain(..executions.len() - 100);
        }
        InspectionAnalytics {
            session_id,
            records: self.records,
            execution_count,
            tool_calls,
            failed_tool_calls,
            last_activity_ms: self.last_activity_ms,
            context_request_count,
            context_requests,
            compactions,
            tools,
            activity: buckets,
            executions,
        }
    }
}

pub async fn inspect(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<InspectionAnalytics>> {
    valid_id(&id)?;
    let mut reader = state
        .runs
        .store
        .rollout(&id)
        .await
        .map_err(ApiError::storage)?;
    let mut aggregate = Aggregate::default();
    while let Some(record) = reader.next().await.map_err(ApiError::storage)? {
        aggregate.add(record);
    }
    Ok(Json(aggregate.finish(id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kyler_core::{
        Session,
        context::{ContextMemory, EventAuthor},
    };
    use kyler_protocol::api::inspection::{ContextBudget, ContextRequestSnapshot};
    use serde_json::json;

    fn record(seq: u64, payload: RolloutPayload) -> RolloutRecord {
        RolloutRecord {
            schema_version: 1,
            session_id: "fixture".into(),
            execution_id: Some("execution".into()),
            seq,
            timestamp_ms: seq as i64 * 1000,
            payload,
        }
    }
    fn event(parts: Vec<ContentItem>) -> Event {
        Event::new("execution", EventAuthor::default(), parts)
    }
    fn call(id: &str) -> ContentItem {
        ContentItem::ToolCall {
            tool_call_id: id.into(),
            name: "apply_patch".into(),
            arguments: json!({"input":"PRIVATE_ARGUMENT"}),
        }
    }
    fn result(id: &str, failed: bool) -> ContentItem {
        ContentItem::ToolResult {
            tool_call_id: id.into(),
            name: "apply_patch".into(),
            status: if failed {
                ToolResultStatus::Error
            } else {
                ToolResultStatus::Success
            },
            content: "PRIVATE_OUTPUT".into(),
            model_content: None,
        }
    }

    #[test]
    fn repeated_event_checkpoints_are_not_counted_as_new_calls_or_results() {
        let mut aggregate = Aggregate::default();
        let call = event(vec![call("same-id")]);
        let result = event(vec![result("same-id", true)]);
        aggregate.add(record(
            1,
            RolloutPayload::EventRecorded {
                event: call.clone(),
            },
        ));
        aggregate.add(record(
            2,
            RolloutPayload::EventRecorded {
                event: result.clone(),
            },
        ));
        aggregate.add(record(3, RolloutPayload::EventRecorded { event: call }));
        aggregate.add(record(4, RolloutPayload::EventRecorded { event: result }));
        let analytics = aggregate.finish("fixture".into());
        assert_eq!(analytics.records, 4);
        assert_eq!(analytics.tool_calls, 1);
        assert_eq!(analytics.failed_tool_calls, 1);
        assert_eq!(analytics.tools[0].pending, 0);
        let encoded = serde_json::to_string(&analytics).unwrap();
        assert!(!encoded.contains("PRIVATE_ARGUMENT"));
        assert!(!encoded.contains("PRIVATE_OUTPUT"));
    }
    #[test]
    fn legacy_base_snapshots_and_reused_call_ids_preserve_order_and_totals() {
        let mut aggregate = Aggregate::default();
        let mut session = Session::new();
        session.events = vec![
            event(vec![call("reused")]),
            event(vec![result("reused", false)]),
            event(vec![call("reused")]),
            event(vec![result("reused", true)]),
        ];
        aggregate.add(record(
            1,
            RolloutPayload::SessionCreated {
                session: Box::new(session),
            },
        ));
        let analytics = aggregate.finish("fixture".into());
        assert_eq!(analytics.records, 1);
        assert_eq!(analytics.tool_calls, 2);
        assert_eq!(analytics.tools[0].succeeded, 1);
        assert_eq!(analytics.tools[0].failed, 1);
    }
    #[test]
    fn context_samples_are_deduplicated_updated_and_bounded() {
        let mut aggregate = Aggregate::default();
        for step in 1..=350 {
            let memory = ContextMemory {
                last_request: Some(ContextRequestSnapshot {
                    execution_id: "execution".into(),
                    step,
                    timestamp_ms: step as i64,
                    budget: ContextBudget {
                        model: "fixture".into(),
                        context_tokens: 1000,
                        sliding_window_tokens: 800,
                        input_limit: 900,
                        output_tokens: 80,
                        safety_tokens: 20,
                    },
                    estimated_input_tokens: 100,
                    provider_input_tokens: None,
                    components: Vec::new(),
                    retrieved_memory_keys: Vec::new(),
                    visible_events: 1,
                    window_start: 0,
                }),
                ..Default::default()
            };
            aggregate.add(record(
                step as u64 * 2,
                RolloutPayload::ContextUpdated {
                    memory: memory.clone(),
                },
            ));
            let mut updated = memory;
            updated.last_request.as_mut().unwrap().provider_input_tokens = Some(150);
            aggregate.add(record(
                step as u64 * 2 + 1,
                RolloutPayload::ContextUpdated { memory: updated },
            ));
        }
        let analytics = aggregate.finish("fixture".into());
        assert_eq!(analytics.context_request_count, 350);
        assert_eq!(analytics.context_requests.len(), 300);
        assert_eq!(analytics.context_requests[0].step, 51);
        assert!(
            analytics
                .context_requests
                .iter()
                .all(|point| !point.estimated && point.input_tokens == 150)
        );
    }
    #[test]
    fn whole_history_counts_are_independent_of_chart_and_timeline_limits() {
        let mut aggregate = Aggregate::default();
        for index in 0..90 {
            let id = format!("call-{index}");
            let mut called = event(vec![ContentItem::ToolCall {
                tool_call_id: id.clone(),
                name: format!("tool-{}", index % 25),
                arguments: json!({}),
            }]);
            called.timestamp = index as i64 * 86400;
            let mut finished = event(vec![result(&id, index % 3 == 0)]);
            finished.timestamp = called.timestamp;
            aggregate.add(record(
                index * 2 + 1,
                RolloutPayload::EventRecorded { event: called },
            ));
            aggregate.add(record(
                index * 2 + 2,
                RolloutPayload::EventRecorded { event: finished },
            ));
        }
        let analytics = aggregate.finish("fixture".into());
        assert_eq!(analytics.records, 180);
        assert_eq!(analytics.tool_calls, 90);
        assert_eq!(analytics.failed_tool_calls, 30);
        assert_eq!(analytics.tools.len(), 21);
        assert_eq!(
            analytics.tools.iter().map(|tool| tool.calls).sum::<u64>(),
            90
        );
        assert!(analytics.activity.len() <= 24);
    }
}
