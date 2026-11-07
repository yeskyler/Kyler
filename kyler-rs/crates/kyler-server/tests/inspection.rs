#[allow(dead_code)]
mod support;

use std::{sync::atomic::Ordering, time::Duration};

use serde_json::{Value, json};

use kyler_config::{SecretString, ServerSettings};
use kyler_core::{
    context::{ContentItem, Event, EventAuthor, Role, ToolResultStatus},
    memory::MemoryStore,
};
use kyler_protocol::api::inspection::{ContextInspection, RolloutDetail, RolloutPage};
use support::Harness;

#[tokio::test]
async fn inspection_is_session_scoped_committed_and_does_not_call_the_model() {
    let mut h = Harness::new(ServerSettings {
        access_token: SecretString::new("inspection-token"),
        ..Default::default()
    })
    .await;
    let id = h.session().await;
    assert_eq!(
        reqwest::get(format!("{}/v1/sessions/{id}/context", h.url))
            .await
            .unwrap()
            .status(),
        401
    );
    let inspect = format!("{}/v1/sessions/{id}", h.url);
    let initial: ContextInspection = h
        .client
        .get(format!("{inspect}/context"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(initial.latest_request.is_none());
    let mut session = h.store.load(&id).await.unwrap();
    session
        .state
        .insert("secret-state".into(), json!("DO_NOT_EXPOSE_STATE"));
    session.memory.summary = Some("A persisted summary".into());
    session.memory.compression_count = 1;
    session.events.push(Event::new(
        "fixture-execution",
        EventAuthor {
            name: "user".into(),
        },
        vec![ContentItem::Message {
            role: Role::User,
            content: "Inspect fixture".into(),
        }],
    ));
    session.events.push(Event::new(
        "fixture-execution",
        EventAuthor {
            name: "tool".into(),
        },
        vec![
            ContentItem::ToolCall {
                tool_call_id: "sample".into(),
                name: "read_file".into(),
                arguments: json!({"api_key":"DO_NOT_EXPOSE_CREDENTIAL","path":"readme.md"}),
            },
            ContentItem::ToolResult {
                tool_call_id: "sample".into(),
                name: "read_file".into(),
                status: ToolResultStatus::Success,
                content: "Original complete output".into(),
                model_content: Some("Compact output".into()),
            },
        ],
    ));
    h.store.save(&mut session).await.unwrap();
    let memory = MemoryStore::new(h.root.path().join("_memory"));
    let scope = MemoryStore::scope(&session);
    for index in 0..20 {
        memory
            .put(
                &scope,
                &format!("fact-{index}"),
                "Saved fact",
                vec![],
                None,
                &id,
            )
            .unwrap();
    }
    let memory: Value = h
        .client
        .get(format!("{inspect}/memory/inspect"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(memory["summary"], "A persisted summary");
    assert_eq!(
        memory["entries"].as_array().unwrap().len(),
        20,
        "inspection lists all entries rather than the retrieval limit of 16"
    );
    let other = h.session().await;
    let other_memory: Value = h
        .client
        .get(format!("{}/v1/sessions/{other}/memory/inspect", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(other_memory["entries"].as_array().unwrap().is_empty());
    let mut cursor = 0;
    let mut sequences = Vec::new();
    let mut tool = None;
    loop {
        let page: RolloutPage = h
            .client
            .get(format!("{inspect}/rollout?cursor={cursor}&limit=2"))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        for record in page.items {
            if record.kind == "tool" {
                tool = Some(record.seq);
            }
            let response = h
                .client
                .get(format!("{inspect}/rollout/{}", record.seq))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .text()
                .await
                .unwrap();
            assert!(!response.contains("DO_NOT_EXPOSE_CREDENTIAL"));
            assert!(!response.contains("DO_NOT_EXPOSE_STATE"));
            assert!(record.seq > cursor);
            sequences.push(record.seq);
        }
        cursor = page.cursor;
        if page.next_cursor.is_none() {
            break;
        }
    }
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    let result: RolloutDetail = h
        .client
        .get(format!("{inspect}/rollout/{}", tool.unwrap()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let text = serde_json::to_string(&result).unwrap();
    assert!(
        text.contains("Original complete output")
            && text.contains("Compact output")
            && text.contains("[REDACTED]")
    );
    assert_eq!(
        h.client
            .get(format!("{inspect}/rollout?limit=0"))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        h.client
            .get(format!("{inspect}/rollout/999999"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(h.calls.load(Ordering::SeqCst), 0);
    h.close().await;
}

#[tokio::test]
async fn request_snapshot_is_visible_while_streaming_and_survives_reloading() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let id = h.session().await;
    let run = h.run(&id, "slow", "inspect-stream").await;
    h.first_delta_then_disconnect(&run.run_id).await;
    let url = format!("{}/v1/sessions/{id}/context", h.url);
    let during: ContextInspection = h
        .client
        .get(&url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let request = during.latest_request.unwrap();
    assert!(request.estimated_input_tokens > 0);
    assert_eq!(
        request
            .components
            .iter()
            .map(|item| item.estimated_tokens)
            .sum::<usize>(),
        request.estimated_input_tokens
    );
    assert_eq!(
        request.execution_id,
        h.info(&run.run_id).await.execution_id.unwrap()
    );
    assert!(request.provider_input_tokens.is_none());
    assert!(during.cumulative_input_tokens < request.estimated_input_tokens as u64);
    let events: RolloutPage = h
        .client
        .get(format!("{}/v1/sessions/{id}/rollout", h.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        events
            .items
            .iter()
            .any(|item| item.title == "Context checkpoint")
    );
    h.release.send_replace(true);
    h.terminal(&run.run_id).await;
    let completed: ContextInspection = h
        .client
        .get(&url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        completed.latest_request.unwrap().provider_input_tokens,
        Some(10)
    );
    let stored = h.store.load(&id).await.unwrap();
    assert_eq!(
        stored.memory.last_request.unwrap().provider_input_tokens,
        Some(10)
    );
    assert_eq!(
        h.calls.load(Ordering::SeqCst),
        1,
        "inspection must not trigger another completion"
    );
    tokio::time::sleep(Duration::from_millis(5)).await;
    h.close().await;
}
