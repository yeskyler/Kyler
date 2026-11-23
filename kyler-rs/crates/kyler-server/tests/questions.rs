mod support;
use kyler_config::ServerSettings;
use kyler_protocol::api::run::RunStatus;
use serde_json::json;
use support::Harness;

#[tokio::test]
async fn plain_chat_clarification_publishes_the_draft() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let session: kyler_protocol::api::session::SessionSummary = h
        .client
        .post(format!("{}/v1/sessions", h.url))
        .json(&json!({"draft":true}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(session.draft);
    let run = h
        .run(
            &session.session_id,
            "deliver:Choose an output format.",
            "draft-question",
        )
        .await;
    assert_eq!(h.terminal(&run.run_id).await.status, RunStatus::Completed);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let current: kyler_protocol::api::session::SessionSummary = h
                .client
                .get(format!("{}/v1/sessions/{}", h.url, session.session_id))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if !current.draft {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(h.requests.lock().unwrap().iter().all(|request| {
        request["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|tool| tool["function"]["name"] != "ask_question")
    }));
    let history = h.store.load(&session.session_id).await.unwrap();
    assert!(
        serde_json::to_string(&history.events)
            .unwrap()
            .contains("Choose an output format.")
    );
    assert!(!h.events(&run.run_id, 0).await.iter().any(|event| {
        matches!(
            event.payload,
            kyler_protocol::api::event::EventPayload::QuestionUpdated { .. }
        )
    }));
    h.close().await;
}
