//! Shared presentation of durable history for terminal and service adapters.
use crate::{Session, context::ContentItem};

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use kyler_protocol::api::history::{HistoryContent, HistoryEvent, HistoryExecution, HistoryPage};
pub fn history(session: &Session, cursor: usize, limit: usize) -> Result<HistoryPage> {
    if cursor > session.events.len() {
        anyhow::bail!("Pagination cursor is out of range.");
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
                    redact(&mut arguments);
                    let summary = presentation.as_ref().and_then(|metadata| tool_summary(name, &arguments, &metadata.source));
                    if arguments.to_string().len() > 8192 { arguments = serde_json::json!({"preview":prefix(&arguments.to_string(),8192),"truncated":true}); }
                    HistoryContent::ToolCall { call_id: tool_call_id.clone(), name: name.clone(), arguments, presentation, summary }
                }
                ContentItem::ToolResult { tool_call_id, name, status, content, .. } => HistoryContent::ToolResult { call_id: tool_call_id.clone(), name: name.clone(), status: format!("{status:?}").to_ascii_lowercase(), output: prefix(content, 32000), output_bytes: content.len(), truncated: content.len() > 32000 },
            });
        }
        let projected = HistoryEvent {
            id: event.id.clone(),
            execution_id: event.execution_id.clone(),
            timestamp_ms: event.timestamp.saturating_mul(1000),
            content,
        };
        let size = serde_json::to_vec(&projected)?.len();
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
    Ok(HistoryPage {
        events,
        executions,
        next_cursor: (end < session.events.len()).then_some(end),
    })
}
pub fn tool_summary(
    name: &str,
    arguments: &serde_json::Value,
    source: &kyler_protocol::activity::ToolSource,
) -> Option<String> {
    if !matches!(source, kyler_protocol::activity::ToolSource::Builtin) {
        return None;
    }
    let value = match name {
        "read_file" => arguments["file_path"].as_str(),
        "list_dir" => arguments["dir_path"].as_str(),
        "grep_files" => arguments["path"].as_str(),
        "run_command" => arguments["program"].as_str(),
        _ => None,
    }?;
    Some(prefix(
        &value.split_whitespace().collect::<Vec<_>>().join(" "),
        160,
    ))
}
pub fn prefix(text: &str, max_bytes: usize) -> String {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
pub fn redact(value: &mut serde_json::Value) {
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

pub fn tool_details(
    snapshot: &Session,
    call: &str,
) -> Result<kyler_protocol::api::tools::ToolDetails> {
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
        .ok_or_else(|| anyhow::anyhow!("tool call not found"))?;
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
        arguments =
            serde_json::json!({"preview":prefix(&arguments.to_string(),8192),"truncated":true});
    }
    Ok(kyler_protocol::api::tools::ToolDetails {
        call_id: call.to_owned(),
        execution_id: found.0,
        name: found.1,
        arguments,
        status,
        output: prefix(&output, 32000),
        output_bytes: bytes,
        truncated: bytes > 32000,
    })
}
