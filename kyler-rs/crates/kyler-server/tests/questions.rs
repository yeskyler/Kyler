mod support;
use kyler_config::ServerSettings;
use kyler_protocol::api::{questions::*, run::RunStatus};
use serde_json::json;
use support::Harness;

async fn pending(h: &Harness, session: &str) -> QuestionRecord {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let records: Vec<QuestionRecord> = h
                .client
                .get(format!("{}/v1/sessions/{session}/questions", h.url))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            if let Some(record) = records
                .into_iter()
                .find(|record| record.status == QuestionStatus::Pending)
            {
                return record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn question_wait_releases_capacity_and_answer_resumes_with_durable_history() {
    let mut h = Harness::new(ServerSettings {
        max_concurrent_runs: 1,
        run_timeout_seconds: 1,
        ..Default::default()
    })
    .await;
    let session = h.session().await;
    let other = h.session().await;
    let run = h.run(&session, "question", "question").await;
    let question = pending(&h, &session).await;
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let info: kyler_protocol::api::run::RunInfo = h
        .client
        .get(format!("{}/v1/runs/{}", h.url, run.run_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info.status, RunStatus::Running);
    let other_run = h.run(&other, "fast", "other").await;
    assert_eq!(
        h.terminal(&other_run.run_id).await.status,
        RunStatus::Completed
    );
    let response =
        json!({"action":"submit","answers":{"format":{"text":"HTML presentation","selected":[]}}});
    assert_eq!(
        h.client
            .post(format!(
                "{}/v1/sessions/{other}/questions/{}/response",
                h.url, question.id
            ))
            .json(&response)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let url = format!(
        "{}/v1/sessions/{session}/questions/{}/response",
        h.url, question.id
    );
    let answer: QuestionRecord = h
        .client
        .post(&url)
        .json(&response)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(answer.status, QuestionStatus::Answered);
    assert_eq!(
        h.client
            .post(&url)
            .json(&response)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        h.client
            .post(&url)
            .json(&json!({"action":"cancel"}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(h.terminal(&run.run_id).await.status, RunStatus::Completed);
    let events = h.events(&run.run_id, 0).await;
    assert!(events.iter().any(|event| matches!(&event.payload,kyler_protocol::api::event::EventPayload::QuestionUpdated { question } if question.status == QuestionStatus::Pending)));
    assert!(h.store.load(&session).await.unwrap().events.iter().flat_map(|event| &event.content).any(|item| matches!(item, kyler_core::context::ContentItem::ToolResult { name,content,.. } if name=="ask_question" && content.contains("HTML presentation"))));
    h.restart().await;
    let records: Vec<QuestionRecord> = h
        .client
        .get(format!("{}/v1/sessions/{session}/questions", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(records, vec![answer]);
    h.close().await;
}
#[tokio::test]
async fn cancelling_question_continues_without_consent_and_cancelling_run_closes_question() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let session = h.session().await;
    let run = h.run(&session, "question", "dismiss").await;
    let question = pending(&h, &session).await;
    let record: QuestionRecord = h
        .client
        .post(format!(
            "{}/v1/sessions/{session}/questions/{}/response",
            h.url, question.id
        ))
        .json(&json!({"action":"cancel"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(record.status, QuestionStatus::Dismissed);
    assert!(record.answers.is_empty());
    assert_eq!(h.terminal(&run.run_id).await.status, RunStatus::Completed);
    let session = h.session().await;
    let run = h.run(&session, "question", "cancel").await;
    let question = pending(&h, &session).await;
    h.client
        .post(format!("{}/v1/runs/{}/cancel", h.url, run.run_id))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(h.terminal(&run.run_id).await.status, RunStatus::Cancelled);
    let records: Vec<QuestionRecord> = h
        .client
        .get(format!("{}/v1/sessions/{session}/questions", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        records
            .iter()
            .find(|record| record.id == question.id)
            .unwrap()
            .status,
        QuestionStatus::Cancelled
    );
    assert!(h.events(&run.run_id, 0).await.iter().any(|event| matches!(&event.payload, kyler_protocol::api::event::EventPayload::QuestionUpdated { question } if question.status == QuestionStatus::Cancelled)));
    h.close().await;
}

#[tokio::test]
async fn asking_a_question_publishes_a_draft_before_the_answer_arrives() {
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
        .run(&session.session_id, "question", "draft-question")
        .await;
    pending(&h, &session.session_id).await;
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
    h.client
        .post(format!("{}/v1/runs/{}/cancel", h.url, run.run_id))
        .send()
        .await
        .unwrap();
    h.terminal(&run.run_id).await;
    h.close().await;
}
