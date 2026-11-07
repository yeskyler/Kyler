mod support;

use support::Harness;

use std::{sync::atomic::Ordering, time::Duration};

use serde_json::{Value, json};
use tokio::net::TcpListener;

use kyler_config::{SecretString, ServerSettings};
use kyler_protocol::api::{
    ServerInfo, SessionUsage,
    attachments::AttachmentInfo,
    event::EventPayload,
    run::{RunAccepted, RunStatus},
    session::{SessionPage, SessionSummary},
};
use kyler_server::Server;

#[tokio::test]
async fn streaming_disconnect_resume_and_final_checkpoint() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let session = h.session().await;
    let run = h.run(&session, "slow", "first").await;
    let cursor = h.first_delta_then_disconnect(&run.run_id).await;
    assert_eq!(h.info(&run.run_id).await.status, RunStatus::Running);
    h.release.send_replace(true);
    let info = h.terminal(&run.run_id).await;
    assert_eq!(info.output.as_deref(), Some("Hello"));
    assert_eq!(info.usage.unwrap().total_tokens, 12);
    let events = h.events(&run.run_id, cursor).await;
    assert!(events.iter().all(|e| e.seq > cursor));
    assert!(events.windows(2).all(|e| e[0].seq + 1 == e[1].seq));
    assert!(matches!(
        events.last().unwrap().payload,
        EventPayload::RunCompleted { .. }
    ));
    let saved = h.store.load(&session).await.unwrap();
    assert_eq!(
        saved.execution.unwrap().status,
        kyler_core::ExecutionStatus::Completed
    );
    assert!(h.root.path().join(&session).join("rollout.jsonl").is_file());
    h.close().await;
}

#[tokio::test]
async fn separate_sessions_run_concurrently_and_cancel_independently() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let first = h.session().await;
    let second = h.session().await;
    let first_run = h.run(&first, "slow", "first-parallel").await;
    h.first_delta_then_disconnect(&first_run.run_id).await;
    let second_run = h.run(&second, "slow", "second-parallel").await;
    h.first_delta_then_disconnect(&second_run.run_id).await;
    assert_eq!(h.info(&first_run.run_id).await.status, RunStatus::Running);
    assert_eq!(h.info(&second_run.run_id).await.status, RunStatus::Running);
    assert_eq!(h.calls.load(Ordering::SeqCst), 2);
    h.client
        .post(format!("{}/v1/runs/{}/cancel", h.url, first_run.run_id))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        h.terminal(&first_run.run_id).await.status,
        RunStatus::Cancelled
    );
    assert_eq!(h.info(&second_run.run_id).await.status, RunStatus::Running);
    h.release.send_replace(true);
    assert_eq!(
        h.terminal(&second_run.run_id).await.status,
        RunStatus::Completed
    );
    h.close().await;
}

#[tokio::test]
async fn idempotency_session_exclusion_and_global_capacity() {
    let mut h = Harness::new(ServerSettings {
        max_concurrent_runs: 1,
        ..Default::default()
    })
    .await;
    let session = h.session().await;
    let other = h.session().await;
    let run = h.run(&session, "slow", "same").await;
    h.first_delta_then_disconnect(&run.run_id).await;
    assert_eq!(h.run(&session, "slow", "same").await.run_id, run.run_id);
    for (id, input, key, status) in [
        (&session, "different", "same", 409),
        (&session, "slow", "new", 409),
        (&other, "hello", "other", 429),
    ] {
        let response = h
            .client
            .post(format!("{}/v1/sessions/{id}/runs", h.url))
            .json(&json!({"input":input,"client_request_id":key}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    assert_eq!(
        h.client
            .delete(format!("{}/v1/sessions/{session}", h.url))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(h.calls.load(Ordering::SeqCst), 1);
    h.client
        .post(format!("{}/v1/runs/{}/cancel", h.url, run.run_id))
        .send()
        .await
        .unwrap();
    assert_eq!(h.terminal(&run.run_id).await.status, RunStatus::Cancelled);
    let response = h
        .client
        .post(format!("{}/v1/runs/{}/cancel", h.url, run.run_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    h.close().await;
}

#[tokio::test]
async fn slow_subscribers_do_not_block_runs_and_expired_cursors_are_explicit() {
    let mut h = Harness::new(ServerSettings {
        event_buffer_max_events: 4,
        event_buffer_max_bytes: 4096,
        ..Default::default()
    })
    .await;
    let session = h.session().await;
    let run = h.run(&session, "long", "long").await;
    let info = h.terminal(&run.run_id).await;
    assert_eq!(info.status, RunStatus::Completed);
    let response = h
        .client
        .get(format!("{}/v1/runs/{}/events", h.url, run.run_id))
        .header("last-event-id", "0")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 410);
    let response = h
        .client
        .get(format!("{}/v1/runs/{}/events", h.url, run.run_id))
        .header("last-event-id", info.last_event_seq + 1)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    h.close().await;
}

#[tokio::test]
async fn timeout_and_shutdown_cancel_and_save_active_runs() {
    let mut h = Harness::new(ServerSettings {
        run_timeout_seconds: 1,
        ..Default::default()
    })
    .await;
    let session = h.session().await;
    let run = h.run(&session, "slow", "timeout").await;
    let info = h.terminal(&run.run_id).await;
    assert_eq!(info.status, RunStatus::Failed);
    assert_eq!(info.error.unwrap().code, "run_timeout");
    assert_eq!(
        h.store
            .load(&session)
            .await
            .unwrap()
            .execution
            .unwrap()
            .status,
        kyler_core::ExecutionStatus::Cancelled
    );
    let listed: SessionPage = h
        .client
        .get(format!("{}/v1/sessions", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        listed
            .sessions
            .iter()
            .find(|s| s.session_id == session)
            .unwrap()
            .status,
        Some(RunStatus::Failed)
    );
    let session = h.session().await;
    let run = h.run(&session, "slow", "shutdown").await;
    h.first_delta_then_disconnect(&run.run_id).await;
    h.close().await;
    assert_eq!(
        h.store
            .load(&session)
            .await
            .unwrap()
            .execution
            .unwrap()
            .status,
        kyler_core::ExecutionStatus::Cancelled
    );
}

#[tokio::test]
async fn restart_retains_results_but_does_not_promise_token_replay() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let session = h.session().await;
    let run = h.run(&session, "hello", "restart").await;
    h.terminal(&run.run_id).await;
    h.restart().await;
    assert_eq!(h.info(&run.run_id).await.output.as_deref(), Some("Hello"));
    assert_eq!(
        h.client
            .get(format!("{}/v1/runs/{}/events", h.url, run.run_id))
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
    assert_eq!(
        h.client
            .delete(format!("{}/v1/sessions/{session}", h.url))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        h.client
            .get(format!("{}/v1/runs/{}", h.url, run.run_id))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert!(
        !h.root
            .path()
            .join("_runs")
            .join(format!("{}.json", run.run_id))
            .exists()
    );
    h.close().await;
}

#[tokio::test]
async fn authentication_origin_limits_errors_and_tool_payload_privacy() {
    let mut h = Harness::new(ServerSettings {
        access_token: SecretString::new("test-access-token"),
        max_request_bytes: 1024,
        allowed_origins: vec!["https://example.com".into()],
        ..Default::default()
    })
    .await;
    let plain = reqwest::Client::new();
    assert_eq!(
        plain
            .get(format!("{}/healthz", h.url))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        plain
            .post(format!("{}/v1/sessions", h.url))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        h.client
            .post(format!("{}/v1/sessions", h.url))
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let preflight = plain
        .request(reqwest::Method::OPTIONS, format!("{}/v1/sessions", h.url))
        .header("origin", "https://example.com")
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            "authorization,content-type",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        preflight.headers()["access-control-allow-origin"],
        "https://example.com"
    );
    let session = h.session().await;
    for body in [
        json!({"input":"x".repeat(2000)}),
        json!({"input":"hello","workspace_root":"/"}),
    ] {
        let response = h
            .client
            .post(format!("{}/v1/sessions/{session}/runs", h.url))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert!(response.status() == 413 || response.status() == 400);
    }
    let run = h.run(&session, "tool", "tool").await;
    h.terminal(&run.run_id).await;
    let events = h.events(&run.run_id, 0).await;
    let json = serde_json::to_string(&events).unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e.payload, EventPayload::ToolFinished { .. }))
    );
    assert!(!json.contains("PRIVATE_ARGUMENT") && !json.contains("PRIVATE_TOOL_RESULT"));
    let response = h
        .client
        .get(format!("{}/v1/sessions/{session}", h.url))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(response.get("state").is_none() && response.get("environment").is_none());
    let response = h
        .client
        .get(format!("{}/v1/sessions/{session}/messages?limit=1", h.url))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(response["messages"].as_array().unwrap().len(), 1);
    let run = h.run(&session, "broken", "broken").await;
    let info = h.terminal(&run.run_id).await;
    assert_eq!(info.status, RunStatus::Failed);
    assert_eq!(info.error.unwrap().code, "model_invalid_response");
    h.close().await;
}

#[tokio::test]
async fn interrupted_manifests_are_recovered_without_resubmitting_input() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let session = h.session().await;
    let run = h.run(&session, "hello", "crash").await;
    let mut manifest = h.terminal(&run.run_id).await;
    h.close().await;
    let calls = h.calls.load(Ordering::SeqCst);
    let mut saved = h.store.load(&session).await.unwrap();
    saved
        .begin_execution("DO_NOT_AUTOMATICALLY_REPLAY")
        .unwrap();
    manifest.execution_id = Some(saved.execution.as_ref().unwrap().execution_id.clone());
    h.store.save(&mut saved).await.unwrap();
    manifest.status = RunStatus::Running;
    manifest.finished_at = None;
    manifest.output = None;
    std::fs::write(
        h.root
            .path()
            .join("_runs")
            .join(format!("{}.json", manifest.run_id)),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    h.restart().await;
    let recovered = h.info(&run.run_id).await;
    assert_eq!(recovered.status, RunStatus::Failed);
    assert_eq!(recovered.error.unwrap().code, "server_restarted");
    assert_eq!(h.calls.load(Ordering::SeqCst), calls);
    assert_eq!(
        h.store
            .load(&session)
            .await
            .unwrap()
            .execution
            .unwrap()
            .status,
        kyler_core::ExecutionStatus::Cancelled
    );
    h.close().await;
}

#[tokio::test]
async fn terminal_retention_and_single_server_ownership_are_enforced() {
    let mut h = Harness::new(ServerSettings {
        max_concurrent_runs: 1,
        max_retained_runs: 1,
        ..Default::default()
    })
    .await;
    assert!(
        kyler_server::Server::with_agent(h.settings.clone(), h.agent.clone(), h.store.clone())
            .await
            .is_err()
    );
    let session = h.session().await;
    let first = h.run(&session, "hello", "first").await;
    h.terminal(&first.run_id).await;
    let second = h.run(&session, "hello", "second").await;
    h.terminal(&second.run_id).await;
    assert_eq!(
        h.client
            .get(format!("{}/v1/runs/{}", h.url, first.run_id))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert!(
        !h.root
            .path()
            .join("_runs")
            .join(format!("{}.json", first.run_id))
            .exists()
    );
    h.close().await;
}

#[tokio::test]
async fn sse_subscriptions_are_limited_and_released_on_disconnect() {
    let mut h = Harness::new(ServerSettings {
        max_subscribers_per_run: 1,
        ..Default::default()
    })
    .await;
    let session = h.session().await;
    let run = h.run(&session, "slow", "subscriber").await;
    let first = h
        .client
        .get(format!("{}/v1/runs/{}/events", h.url, run.run_id))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    let second = h
        .client
        .get(format!("{}/v1/runs/{}/events", h.url, run.run_id))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 429);
    drop(first);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let response = h
                .client
                .get(format!("{}/v1/runs/{}/events", h.url, run.run_id))
                .send()
                .await
                .unwrap();
            if response.status() == 200 {
                drop(response);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    h.close().await;
}

#[tokio::test]
async fn actual_listener_cannot_bypass_loopback_authentication_requirement() {
    let mut h = Harness::new(ServerSettings::default()).await;
    h.close().await;
    let server = Server::with_agent(h.settings.clone(), h.agent.clone(), h.store.clone())
        .await
        .unwrap();
    let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
    let result = server.serve(listener, std::future::pending()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn runtime_metadata_and_session_usage_are_server_authoritative_and_authenticated() {
    let mut h = Harness::new(ServerSettings {
        access_token: SecretString::new("metadata-token"),
        ..Default::default()
    })
    .await;
    let anonymous = reqwest::Client::new()
        .get(format!("{}/v1/info", h.url))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);
    let info: ServerInfo = h
        .client
        .get(format!("{}/v1/info", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info.model, "test");
    assert_eq!(info.context_window_tokens, 128000);
    assert_eq!(info.sliding_window_tokens, Some(76800));
    assert!(info.workspace_root.is_none());
    let session = h.session().await;
    for index in 0..2 {
        let run = h.run(&session, "hello", &format!("usage-{index}")).await;
        h.terminal(&run.run_id).await;
        let usage: SessionUsage = h
            .client
            .get(format!("{}/v1/sessions/{session}/usage", h.url))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(usage.total_tokens, (index + 1) * 12);
        assert_eq!(usage.prompt_tokens, (index + 1) * 10);
        assert_eq!(usage.completion_tokens, (index + 1) * 2);
    }
    h.close().await;
}

#[tokio::test]
async fn image_turn_uses_existing_chat_completions_and_survives_reload() {
    use base64::{Engine, engine::general_purpose::STANDARD};

    let mut h = Harness::new(ServerSettings::default()).await;
    let session = h.session().await;
    let url = format!("{}/v1/sessions/{session}/runs", h.url);
    let mut bytes = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==").unwrap();
    bytes.resize(70_000, 0); // Exercise the image route's larger body limit.
    let request = json!({"input":"", "client_request_id":"picture", "images":[{"name":"screenshot.png","data":STANDARD.encode(bytes)}]});
    let response = h.client.post(&url).json(&request).send().await.unwrap();
    assert_eq!(
        response.status(),
        202,
        "{}",
        response.text().await.unwrap_or_default()
    );
    let run: RunAccepted = response.json().await.unwrap();
    let terminal = h.terminal(&run.run_id).await;
    assert_eq!(
        terminal.status,
        RunStatus::Completed,
        "saved execution: {:?}",
        h.store.load(&session).await.unwrap().execution
    );
    let same: RunAccepted = h
        .client
        .post(&url)
        .json(&request)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(same.run_id, run.run_id);
    let mut changed = request.clone();
    changed["images"][0]["name"] = json!("different.png");
    assert_eq!(
        h.client
            .post(&url)
            .json(&changed)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    {
        let requests = h.requests.lock().unwrap();
        let parts = &requests.last().unwrap()["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "user")
            .unwrap()["content"];
        assert_eq!(parts[0]["type"], "image_url");
        assert!(
            parts[0]["image_url"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
    }
    h.restart().await;
    let url = format!("{}/v1/sessions/{session}/runs", h.url);
    let followup = h.run(&session, "What color?", "followup").await;
    assert_eq!(
        h.terminal(&followup.run_id).await.status,
        RunStatus::Completed
    );
    assert!(
        h.requests
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .to_string()
            .contains("data:image/png;base64,")
    );
    let history = h
        .client
        .get(format!("{}/v1/sessions/{session}/messages", h.url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(history.contains("[Image: screenshot.png]"));
    assert!(!history.contains("base64"));
    for images in [
        json!([{"name":"bad.png","data":"%%%"}]),
        json!([{"name":"bad.png","data":"aGVsbG8="}]),
        json!(vec![json!({"name":"bad.png","data":""}); 5]),
    ] {
        let response = h
            .client
            .post(&url)
            .json(&json!({"input":"test","images":images}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
    }
    h.close().await;
}

#[tokio::test]
async fn provider_image_rejection_has_safe_matching_manifest_and_stream_errors() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let session = h.session().await;
    let run = h.run(&session, "reject-image", "vision-failure").await;
    let info = h.terminal(&run.run_id).await;
    assert_eq!(info.status, RunStatus::Failed);
    let error = info.error.unwrap();
    assert_eq!(error.code, "model_image_unsupported");
    assert!(error.message.contains("/model"));
    let events = h.events(&run.run_id, 0).await;
    assert!(
        matches!(&events.last().unwrap().payload, EventPayload::RunFailed { error } if error.code=="model_image_unsupported")
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::ToolStarted { .. }))
    );
    let public = serde_json::to_string(&events).unwrap();
    assert!(!public.contains("PRIVATE_PROVIDER_VALUE") && !public.contains("PRIVATE_IMAGE"));
    let saved = h.store.load(&session).await.unwrap();
    assert!(
        saved
            .execution
            .unwrap()
            .error
            .unwrap()
            .contains("PRIVATE_PROVIDER_VALUE")
    );
    h.restart().await;
    assert_eq!(
        h.info(&run.run_id).await.error.unwrap().code,
        "model_image_unsupported"
    );
    h.close().await;
}

#[tokio::test]
async fn uploads_bind_to_sessions_and_runs_persist_references_instead_of_base64() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let session = h.session().await;
    let other = h.session().await;
    let response = h
        .client
        .post(format!(
            "{}/v1/sessions/{session}/attachments?filename=report.txt",
            h.url
        ))
        .body("Revenue: 42\nExpense: 12")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let attachment: AttachmentInfo = response.json().await.unwrap();
    let response = h
        .client
        .post(format!("{}/v1/sessions/{other}/runs", h.url))
        .json(&json!({"input":"Analyze","attachment_ids":[attachment.id]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let response = h
        .client
        .post(format!("{}/v1/sessions/{session}/runs", h.url))
        .json(&json!({"input":"Analyze","attachment_ids":[attachment.id]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let accepted: RunAccepted = response.json().await.unwrap();
    assert_eq!(
        h.terminal(&accepted.run_id).await.status,
        RunStatus::Completed
    );
    let saved = h.store.load(&session).await.unwrap();
    let json = serde_json::to_string(&saved.events).unwrap();
    assert!(json.contains(&attachment.id));
    assert!(!json.contains("Revenue: 42"));
    assert_eq!(
        h.client
            .delete(format!(
                "{}/v1/sessions/{session}/attachments/{}",
                h.url, attachment.id
            ))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let result = h
        .client
        .get(format!("{}/v1/sessions/{session}/memory", h.url))
        .send()
        .await
        .unwrap();
    assert_eq!(result.status(), 200);
    h.close().await;
}

#[tokio::test]
async fn resume_continues_failed_execution_and_preserves_provenance() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let session = h.session().await;
    let first = h.run(&session, "broken", "first-broken").await;
    assert_eq!(h.terminal(&first.run_id).await.status, RunStatus::Failed);
    let old = h
        .store
        .load(&session)
        .await
        .unwrap()
        .execution
        .unwrap()
        .execution_id;
    let response = h
        .client
        .post(format!("{}/v1/sessions/{session}/runs", h.url))
        .json(&json!({"input":"","resume":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let accepted: RunAccepted = response.json().await.unwrap();
    assert_eq!(
        h.terminal(&accepted.run_id).await.status,
        RunStatus::Completed
    );
    assert_eq!(
        h.store
            .load(&session)
            .await
            .unwrap()
            .execution
            .unwrap()
            .resumed_from,
        Some(old)
    );
    let response = h
        .client
        .post(format!("{}/v1/sessions/{session}/runs", h.url))
        .json(&json!({"input":"","resume":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    h.close().await;
}

#[tokio::test]
async fn tool_details_are_available_separately_without_putting_arguments_into_sse() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let session = h.session().await;
    let run = h.run(&session, "tool", "details").await;
    h.terminal(&run.run_id).await;
    let response = h
        .client
        .get(format!(
            "{}/v1/sessions/{session}/tool-calls/echo-call",
            h.url
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let detail: Value = response.json().await.unwrap();
    assert_eq!(detail["name"], "echo");
    assert!(
        detail["output"]
            .as_str()
            .unwrap()
            .contains("PRIVATE_TOOL_RESULT")
    );
    let events = h.events(&run.run_id, 0).await;
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("PRIVATE_TOOL_RESULT")
    );
    h.close().await;
}

#[tokio::test]
async fn idempotent_admissions_survive_server_restart() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let session = h.session().await;
    let run = h.run(&session, "hello", "durable-id").await;
    h.terminal(&run.run_id).await;
    let calls = h.calls.load(Ordering::SeqCst);
    h.restart().await;
    assert_eq!(
        h.run(&session, "hello", "durable-id").await.run_id,
        run.run_id
    );
    assert_eq!(h.calls.load(Ordering::SeqCst), calls);
    let response = h
        .client
        .post(format!("{}/v1/sessions/{session}/runs", h.url))
        .json(&json!({"input":"changed","client_request_id":"durable-id"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    h.close().await;
}

#[tokio::test]
async fn session_history_reports_live_status_and_refuses_deleting_active_sessions() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let active = h.session().await;
    let idle = h.session().await;
    let run = h.run(&active, "slow", "history-status").await;
    h.first_delta_then_disconnect(&run.run_id).await;
    let page: SessionPage = h
        .client
        .get(format!("{}/v1/sessions?limit=50", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        page.sessions
            .iter()
            .find(|s| s.session_id == active)
            .unwrap()
            .status,
        Some(RunStatus::Running)
    );
    assert_eq!(
        page.sessions
            .iter()
            .find(|s| s.session_id == idle)
            .unwrap()
            .status,
        None
    );
    let details: SessionSummary = h
        .client
        .get(format!("{}/v1/sessions/{active}", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(details.status, Some(RunStatus::Running));
    assert_eq!(
        h.client
            .delete(format!("{}/v1/sessions/{active}", h.url))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(
        h.client
            .delete(format!("{}/v1/sessions/{idle}", h.url))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    h.client
        .post(format!("{}/v1/runs/{}/cancel", h.url, run.run_id))
        .send()
        .await
        .unwrap();
    h.terminal(&run.run_id).await;
    let page: SessionPage = h
        .client
        .get(format!("{}/v1/sessions", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(page.sessions[0].status, Some(RunStatus::Cancelled));
    assert_eq!(
        h.client
            .delete(format!("{}/v1/sessions/{active}", h.url))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let page: SessionPage = h
        .client
        .get(format!("{}/v1/sessions", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(page.sessions.is_empty());
    h.close().await;
}

#[tokio::test]
async fn web_drafts_enter_history_only_after_the_first_model_response() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let draft: SessionSummary = h
        .client
        .post(format!("{}/v1/sessions", h.url))
        .json(&json!({"draft":true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(draft.draft);
    assert!(draft.title.is_none());
    let page: SessionPage = h
        .client
        .get(format!("{}/v1/sessions", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(page.sessions.is_empty());
    let run = h
        .run(&draft.session_id, "wait-before-reply", "draft-promotion")
        .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while h.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let page: SessionPage = h
        .client
        .get(format!("{}/v1/sessions", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(page.sessions.is_empty());
    h.release.send_replace(true);
    h.terminal(&run.run_id).await;
    let page: SessionPage = h
        .client
        .get(format!("{}/v1/sessions", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(page.sessions[0].session_id, draft.session_id);
    assert!(!page.sessions[0].draft);
    assert_eq!(page.sessions[0].title.as_deref(), Some("wait-before-reply"));
    let failed: SessionSummary = h
        .client
        .post(format!("{}/v1/sessions", h.url))
        .json(&json!({"draft":true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let failure = h
        .run(&failed.session_id, "reject-image", "draft-failure")
        .await;
    assert_eq!(h.terminal(&failure.run_id).await.status, RunStatus::Failed);
    let page: SessionPage = h
        .client
        .get(format!("{}/v1/sessions", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page.sessions.len(), 1);
    let hidden: SessionSummary = h
        .client
        .get(format!("{}/v1/sessions/{}", h.url, failed.session_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(hidden.draft);
    h.release.send_replace(false);
    let cancelled: SessionSummary = h
        .client
        .post(format!("{}/v1/sessions", h.url))
        .json(&json!({"draft":true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pending = h
        .run(&cancelled.session_id, "wait-before-reply", "draft-cancel")
        .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while h.calls.load(Ordering::SeqCst) < 3 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    h.client
        .post(format!("{}/v1/runs/{}/cancel", h.url, pending.run_id))
        .send()
        .await
        .unwrap();
    assert_eq!(
        h.terminal(&pending.run_id).await.status,
        RunStatus::Cancelled
    );
    let page: SessionPage = h
        .client
        .get(format!("{}/v1/sessions", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page.sessions.len(), 1);
    h.restart().await;
    let page: SessionPage = h
        .client
        .get(format!("{}/v1/sessions", h.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(
        h.client
            .delete(format!("{}/v1/sessions/{}", h.url, failed.session_id))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    h.close().await;
}
