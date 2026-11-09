mod support;
use support::Harness;

use kyler_config::ServerSettings;
use kyler_protocol::api::{
    history::{HistoryContent, HistoryPage},
    run::RunStatus,
};

#[tokio::test]
async fn structured_history_survives_restart_and_pagination_with_text_tools_and_metadata() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let session = h.session().await;
    let run = h.run(&session, "history-tool", "history").await;
    let result = h.terminal(&run.run_id).await;
    assert_eq!(result.status, RunStatus::Completed);
    let execution = result.execution_id.unwrap();
    let mut cursor = 0;
    let mut events = Vec::new();
    loop {
        let page: HistoryPage = h
            .client
            .get(format!(
                "{}/v1/sessions/{session}/history?limit=1&cursor={cursor}",
                h.url
            ))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(page.events.len(), 1);
        assert!(
            page.executions
                .iter()
                .any(|item| item.execution_id == execution
                    && item.status == "completed"
                    && item.finished_at.is_some())
        );
        events.extend(page.events);
        if let Some(next) = page.next_cursor {
            assert!(next > cursor);
            cursor = next;
        } else {
            break;
        }
    }
    let parts = events
        .iter()
        .flat_map(|event| &event.content)
        .collect::<Vec<_>>();
    assert!(
        matches!(parts[0], HistoryContent::Message { role, text } if role == "user" && text == "history-tool")
    );
    assert!(
        matches!(parts[1], HistoryContent::Message { text, .. } if text == "Inspecting the project before calling a tool.")
    );
    assert!(
        matches!(parts[2], HistoryContent::ToolCall { call_id, arguments, presentation: Some(metadata), .. } if call_id == "history-call" && arguments["api_key"] == "[REDACTED]" && metadata.started_at.is_some() && metadata.finished_at >= metadata.started_at && metadata.step == 1)
    );
    assert!(
        matches!(parts[3], HistoryContent::ToolResult { status, output, .. } if status == "success" && output == "PRIVATE_TOOL_RESULT")
    );
    assert!(
        matches!(parts[4], HistoryContent::Message { role, text } if role == "assistant" && text == "Hello")
    );
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("DO_NOT_EXPOSE")
    );
    h.restart().await;
    let restored: HistoryPage = h
        .client
        .get(format!("{}/v1/sessions/{session}/history?limit=500", h.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&restored.events).unwrap(),
        serde_json::to_value(&events).unwrap()
    );
    let empty: HistoryPage = h
        .client
        .get(format!(
            "{}/v1/sessions/{session}/history?cursor={}&limit=1",
            h.url,
            events.len()
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(empty.events.is_empty() && empty.next_cursor.is_none());
    assert_eq!(
        h.client
            .get(format!(
                "{}/v1/sessions/{session}/history?cursor=999999",
                h.url
            ))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    h.close().await;
}

#[tokio::test]
async fn history_preserves_partial_text_after_cancel_without_inventing_completion() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let session = h.session().await;
    let run = h.run(&session, "slow", "partial-history").await;
    h.first_delta_then_disconnect(&run.run_id).await;
    h.client
        .post(format!("{}/v1/runs/{}/cancel", h.url, run.run_id))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(h.terminal(&run.run_id).await.status, RunStatus::Cancelled);
    let page: HistoryPage = h
        .client
        .get(format!("{}/v1/sessions/{session}/history", h.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(page.events.iter().flat_map(|event| &event.content).any(|item| matches!(item, HistoryContent::Message { role, text } if role == "assistant" && text == "Hel")));
    assert_eq!(page.executions[0].status, "cancelled");
    h.close().await;
}

#[tokio::test]
async fn legacy_events_keep_attachments_and_explicitly_bound_large_results() {
    use kyler_core::{
        ExecutionStatus,
        context::{ContentItem, Event, EventAuthor, ToolResultStatus},
    };
    use kyler_protocol::api::attachments::AttachmentInfo;
    let mut h = Harness::new(ServerSettings::default()).await;
    let id = h.session().await;
    let attachment: AttachmentInfo = h
        .client
        .post(format!(
            "{}/v1/sessions/{id}/attachments?filename=notes.txt",
            h.url
        ))
        .body("original attachment")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut session = h.store.load(&id).await.unwrap();
    session
        .begin_turn("Inspect attachment", Vec::new(), vec![attachment.clone()])
        .unwrap();
    let execution = session.current_execution().unwrap().execution_id.clone();
    session.add_event(Event::new(
        &execution,
        EventAuthor {
            name: "agent".into(),
        },
        vec![ContentItem::ToolCall {
            tool_call_id: "legacy".into(),
            name: "old_tool".into(),
            arguments: serde_json::json!({"password":"private"}),
        }],
    ));
    session.add_event(Event::new(
        &execution,
        EventAuthor {
            name: "agent".into(),
        },
        vec![ContentItem::ToolResult {
            tool_call_id: "legacy".into(),
            name: "old_tool".into(),
            status: ToolResultStatus::Success,
            content: "x".repeat(40000),
            model_content: Some("compressed for model only".into()),
        }],
    ));
    session
        .finish(ExecutionStatus::Completed, Some("Done".into()), None)
        .unwrap();
    h.store.save(&mut session).await.unwrap();
    let page: HistoryPage = h
        .client
        .get(format!("{}/v1/sessions/{id}/history", h.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = page
        .events
        .iter()
        .flat_map(|event| &event.content)
        .collect::<Vec<_>>();
    assert!(items.iter().any(|item| matches!(item, HistoryContent::Attachment { attachment: info } if info.id == attachment.id)));
    assert!(items.iter().any(|item| matches!(item, HistoryContent::ToolCall { presentation: None, arguments, .. } if arguments["password"] == "[REDACTED]")));
    assert!(items.iter().any(|item| matches!(item, HistoryContent::ToolResult { output, output_bytes: 40000, truncated: true, .. } if output.len() == 32000 && !output.contains("compressed"))));
    h.close().await;
}

#[tokio::test]
async fn context_compaction_stream_and_history_replay_share_one_lifecycle() {
    use kyler_core::context::{ContentItem, ContextPolicy, Event, Role};
    use kyler_protocol::api::{compaction::CompactionStatus, event::EventPayload};
    use std::sync::Arc;
    let mut h = Harness::new(ServerSettings::default()).await;
    h.close().await;
    h.agent = Arc::new((*h.agent).clone().with_context_policy(ContextPolicy {
        context_tokens: 128000,
        output_tokens: 1024,
        summary_tokens: 1024,
        safety_tokens: 1024,
        ..Default::default()
    }));
    h.restart().await;
    let session = h.session().await;
    let mut stored = h.store.load(&session).await.unwrap();
    stored.add_event(Event::new(
        "previous",
        stored.author.clone(),
        vec![ContentItem::Message {
            role: Role::User,
            content: "Earlier question".into(),
        }],
    ));
    stored.add_event(Event::new(
        "previous",
        stored.author.clone(),
        vec![ContentItem::Message {
            role: Role::Assistant,
            content: "old conversation ".repeat(80000),
        }],
    ));
    h.store.save(&mut stored).await.unwrap();
    let run = h.run(&session, "deliver:Done", "compaction").await;
    let result = h.terminal(&run.run_id).await;
    let saved = h.store.load(&session).await.unwrap();
    assert_eq!(
        result.status,
        RunStatus::Completed,
        "{:?}; {:?}",
        result.error,
        saved
            .execution
            .as_ref()
            .and_then(|execution| execution.error.as_ref())
    );
    let events = h.events(&run.run_id, 0).await;
    let records = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ContextCompaction { compaction } => Some(compaction),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].id, records[1].id);
    assert_eq!(records[0].status, CompactionStatus::Running);
    assert_eq!(records[1].status, CompactionStatus::Completed);
    let id = records[0].id.clone();
    h.restart().await;
    let history: HistoryPage = h
        .client
        .get(format!("{}/v1/sessions/{session}/history?limit=500", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let retained = history
        .events
        .iter()
        .flat_map(|event| &event.content)
        .filter_map(|item| match item {
            HistoryContent::ContextCompaction { compaction } => Some(compaction),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), 2);
    assert!(retained.iter().all(|record| record.id == id));
    assert_eq!(retained[1].status, CompactionStatus::Completed);
    h.close().await;
}
