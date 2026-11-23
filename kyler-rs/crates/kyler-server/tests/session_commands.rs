mod support;

use kyler_config::ServerSettings;
use kyler_protocol::api::{
    ServerInfo,
    run::{RunAccepted, RunStatus},
};
use serde_json::json;
use support::Harness;

#[tokio::test]
async fn session_commands_admit_compaction_without_treating_chat_text_as_a_command() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let session = h.session().await;
    let url = format!("{}/v1/sessions/{session}/runs", h.url);
    let info: ServerInfo = h
        .client
        .get(format!("{}/v1/info", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!info.subagents_enabled);

    for request in [
        json!({"input":"", "command":{"type":"compact"}}),
        json!({"input":"", "command":{"type":"memory"}}),
        json!({"input":"", "command":{"type":"review","target":{"type":"uncommitted"}}}),
        json!({"input":"draft", "command":{"type":"compact"}}),
    ] {
        let response = h.client.post(&url).json(&request).send().await.unwrap();
        assert_eq!(response.status(), 400, "{}", response.text().await.unwrap());
    }

    let ordinary = h.run(&session, "/compact", "ordinary-chat").await;
    assert_eq!(
        h.terminal(&ordinary.run_id).await.output.as_deref(),
        Some("Hello")
    );

    let command: RunAccepted = h.client.post(url)
        .json(&json!({"input":"", "command":{"type":"compact"}, "client_request_id":"compact-command"}))
        .send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    let result = h.terminal(&command.run_id).await;
    assert_eq!(result.status, RunStatus::Completed, "{:?}", result.error);
    assert_eq!(
        result.output.as_deref(),
        Some("Conversation history compacted.")
    );
    let saved = h.store.load(&session).await.unwrap();
    assert_eq!(
        saved.memory.summary.as_deref(),
        Some("Retained conversation facts.")
    );
    let request = h.requests.lock().unwrap().last().cloned().unwrap();
    assert_ne!(request["stream"], true);
    assert!(request["tools"].is_null());
    h.close().await;
}
