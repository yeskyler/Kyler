//! Synthetic tasks only. No GAIA dataset contents or real model requests are used.
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::{Json, Router, response::IntoResponse, routing::post};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use kyler_evals::{
    config::EvalConfig,
    gaia::runner,
    report,
    runtime::Runtime,
    startup::{self, ModelMode, Scenario, Settings},
};

fn worker() -> PathBuf {
    env!("CARGO_BIN_EXE_kyler-evals").into()
}
fn settings(root: &std::path::Path) -> EvalConfig {
    let mut config = EvalConfig::default();
    config.dataset.directory = root.join("data");
    config.run.results_directory = root.join("results");
    config.run.timeout_seconds = 10;
    std::fs::create_dir_all(config.dataset.directory.join("validation")).unwrap();
    std::fs::create_dir_all(&config.run.results_directory).unwrap();
    config
}
async fn provider(output: &str) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let record = requests.clone();
    let output = output.to_owned();
    let router=Router::new().route("/v1/chat/completions",post(move|Json(request):Json<Value>|{let record=record.clone();let output=output.clone();async move{
        record.lock().unwrap().push(request);
        let text=json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"eval-mock","choices":[{"index":0,"delta":{"role":"assistant","content":output},"finish_reason":null}]});
        let end=json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"eval-mock","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18}});
        ([("content-type","text/event-stream")],format!("data: {text}\n\ndata: {end}\n\ndata: [DONE]\n\n")).into_response()
    }}));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, requests, task)
}

#[tokio::test]
async fn gaia_run_scores_persists_and_resumes_without_exposing_references() {
    let root = tempfile::tempdir().unwrap();
    let config = settings(root.path());
    let split = config.dataset.directory.join("validation");
    let task = json!({"task_id":"synthetic-1","Question":"Return a short answer.","Level":"1","Final answer":"REFERENCE_MUST_STAY_PRIVATE","file_name":"input.txt"});
    std::fs::write(split.join("metadata.jsonl"), format!("{task}\n")).unwrap();
    std::fs::write(split.join("input.txt"), "synthetic attachment").unwrap();
    let (url, requests, server) = provider("FINAL ANSWER: 42").await;
    let app = startup::probe::mock_config(&url);
    let run = runner::run(config.clone(), app.clone(), None, None, worker())
        .await
        .unwrap();
    let results = report::task_results(&run).unwrap();
    assert_eq!(results.len(), 1);
    let result = &results[0];
    assert_eq!(result.execution_status, "completed");
    assert_eq!(result.answer_correct, Some(false));
    assert_eq!(result.total_tokens, 18);
    assert_eq!(result.model_requests, 1);
    assert_eq!(
        result.attachment_mode.as_deref(),
        Some("attachment_api_and_workspace")
    );
    let request = serde_json::to_string(&requests.lock().unwrap()[0]).unwrap();
    assert!(!request.contains("REFERENCE_MUST_STAY_PRIVATE"));
    assert!(!request.contains("Annotator Metadata"));
    let session = result.session_id.as_ref().unwrap();
    let attempt = run.join("tasks/synthetic-1/attempt-1");
    assert!(
        attempt
            .join("sessions")
            .join(session)
            .join("rollout.jsonl")
            .exists()
    );
    assert_eq!(
        std::fs::read_to_string(attempt.join("workspace/input.txt")).unwrap(),
        "synthetic attachment"
    );
    runner::resume(run.clone(), app.clone(), worker(), false)
        .await
        .unwrap();
    assert_eq!(requests.lock().unwrap().len(), 1);
    runner::resume(run.clone(), app.clone(), worker(), true)
        .await
        .unwrap();
    assert_eq!(report::task_results(&run).unwrap().len(), 2);
    std::fs::write(split.join("input.txt"), "changed").unwrap();
    assert!(runner::resume(run, app, worker(), false).await.is_err());
    server.abort();
}

#[tokio::test]
async fn native_eval_environment_denies_dataset_results_and_outside_writes() {
    let root = tempfile::tempdir().unwrap();
    let config = settings(root.path());
    let app = startup::probe::mock_config("http://127.0.0.1:1/v1");
    let private = config.run.results_directory.join("private");
    std::fs::create_dir(&private).unwrap();
    let runtime = Runtime::build(&app, &config, &private, &worker())
        .await
        .unwrap();
    let environment = runtime.agent.execution_environment().unwrap();
    let source = config.dataset.directory.join("validation/answers.txt");
    std::fs::write(&source, "private labels").unwrap();
    let args = json!({"file_path":source});
    assert!(
        environment
            .run_tool("read_file", &args.to_string())
            .await
            .is_err()
    );
    assert!(
        environment
            .policy()
            .filesystem
            .check_write(runtime.workspace.path(), &private.join("overwrite"))
            .is_err()
    );
    assert!(runtime.agent.skill_manager().is_none());
    assert_eq!(
        runtime
            .agent
            .mcp_manager()
            .unwrap()
            .configured_servers()
            .unwrap()
            .len(),
        0
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn startup_subprocess_measures_all_three_scenarios_with_local_mock() {
    let root = tempfile::tempdir().unwrap();
    for scenario in [Scenario::Runtime, Scenario::Server, Scenario::FirstTurn] {
        let directory = startup::run(
            Settings {
                scenario,
                model_mode: ModelMode::Mock,
                repeat: 1,
                warmup: 0,
                timeout_seconds: 30,
                ..Default::default()
            },
            None,
            root.path().into(),
            None,
            worker(),
        )
        .await
        .unwrap();
        let summary: Value =
            serde_json::from_slice(&std::fs::read(directory.join("summary.json")).unwrap())
                .unwrap();
        assert_eq!(
            summary["successful"],
            1,
            "{scenario:?}: {}",
            std::fs::read_to_string(directory.join("results.json")).unwrap()
        );
        let samples: Vec<startup::Sample> =
            serde_json::from_slice(&std::fs::read(directory.join("results.json")).unwrap())
                .unwrap();
        assert!(samples[0].process_to_ready_ms.unwrap() > 0.0);
        if scenario == Scenario::FirstTurn {
            let timing = &samples[0].measurement.as_ref().unwrap().timings;
            assert_eq!(timing.model_requests, 1);
            assert!(timing.first_model_event_ms.unwrap() >= timing.first_request_ms.unwrap());
        }
    }
}

#[tokio::test]
async fn timed_out_gaia_task_is_cancelled_and_saved_without_a_clean_score() {
    let root = tempfile::tempdir().unwrap();
    let mut config = settings(root.path());
    config.run.timeout_seconds = 1;
    let task = json!({"task_id":"timeout","Question":"Synthetic timeout task","Level":1,"Final answer":"42"});
    std::fs::write(
        config.dataset.directory.join("validation/metadata.jsonl"),
        format!("{task}\n"),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let router = Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            "late"
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let run = runner::run(
        config,
        startup::probe::mock_config(&url),
        None,
        None,
        worker(),
    )
    .await
    .unwrap();
    let results = report::task_results(&run).unwrap();
    assert_eq!(results[0].execution_status, "timed_out");
    assert_eq!(results[0].answer_correct, Some(false));
    let session = results[0].session_id.as_ref().unwrap();
    let directory = run.join("tasks/timeout/attempt-1/sessions");
    let store = kyler_core::SessionStore::new(directory).await.unwrap();
    assert_eq!(
        store
            .load(session)
            .await
            .unwrap()
            .current_execution()
            .unwrap()
            .status,
        kyler_core::ExecutionStatus::Cancelled
    );
    server.abort();
}

#[tokio::test]
async fn cold_warm_and_restore_sessions_preserve_measurement_boundaries() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("app.toml");
    std::fs::write(&config, "[skills]\nenabled = false\n").unwrap();
    for (scenario, snapshot) in [
        (Scenario::SessionCold, true),
        (Scenario::SessionWarm, true),
        (Scenario::SessionRestore, true),
        (Scenario::SessionRestore, false),
    ] {
        let directory = startup::run(
            Settings {
                scenario,
                profile: startup::Profile::Minimal,
                repeat: 1,
                warmup: 0,
                timeout_seconds: 30,
                restore_turns: 3,
                restore_snapshot: snapshot,
                ..Default::default()
            },
            Some(config.clone()),
            root.path().join("results"),
            None,
            worker(),
        )
        .await
        .unwrap();
        let samples: Vec<startup::Sample> =
            serde_json::from_slice(&std::fs::read(directory.join("results.json")).unwrap())
                .unwrap();
        let sample = &samples[0];
        let measurement = sample.measurement.as_ref().unwrap();
        assert!(sample.timeline_ms["turn_submitted"] <= sample.timeline_ms["capabilities_ready"]);
        assert!(
            sample.timeline_ms["capabilities_ready"] <= sample.timeline_ms["model_request_started"]
        );
        assert!(
            sample.timeline_ms["model_request_started"] <= sample.timeline_ms["first_model_event"]
        );
        assert!(sample.timeline_ms["first_model_event"] <= sample.timeline_ms["first_text"]);
        assert_eq!(measurement.timings.model_requests, 1);
        assert_eq!(
            measurement.warmup_turn_ms.is_some(),
            scenario == Scenario::SessionWarm
        );
        assert_eq!(
            measurement.restore_ms.is_some(),
            scenario == Scenario::SessionRestore
        );
    }
}

#[tokio::test]
async fn synthetic_mcp_and_skills_profiles_measure_real_loading_work() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("app.toml");
    std::fs::write(&config, "[skills]\nenabled = false\n").unwrap();
    for profile in [startup::Profile::Mcp, startup::Profile::Skills] {
        let directory = startup::run(
            Settings {
                scenario: Scenario::SessionCold,
                profile,
                repeat: 1,
                warmup: 0,
                timeout_seconds: 30,
                mcp_count: 1,
                mcp_delay_ms: 120,
                skill_count: 3,
                ..Default::default()
            },
            Some(config.clone()),
            root.path().join("results"),
            None,
            worker(),
        )
        .await
        .unwrap();
        let samples: Vec<startup::Sample> =
            serde_json::from_slice(&std::fs::read(directory.join("results.json")).unwrap())
                .unwrap();
        let measurement = samples[0].measurement.as_ref().unwrap();
        let names: Vec<_> = measurement
            .timings
            .tool_snapshots
            .values()
            .flatten()
            .collect();
        if profile == startup::Profile::Mcp {
            assert!(measurement.timings.first_request_ms.unwrap() >= 100.0);
            assert!(names.iter().any(|name| name.contains("bench_echo")));
        } else {
            assert!(names.iter().any(|name| name.as_str() == "load_skill"));
            assert!(!measurement.timings.skills_snapshot_ms.is_empty());
        }
        assert!(measurement.degraded.is_empty());
    }
}

#[tokio::test]
async fn optional_connector_failure_is_degraded_but_required_failure_is_not_ready() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("app.toml");
    std::fs::write(&app,"[skills]\nenabled=false\n[mcp.servers.unavailable]\ntransport='streamable_http'\nurl='http://127.0.0.1:1/mcp'\nconnect_timeout_seconds=1\n").unwrap();
    let settings = Settings {
        scenario: Scenario::SessionCold,
        profile: startup::Profile::Default,
        repeat: 1,
        warmup: 0,
        timeout_seconds: 20,
        ..Default::default()
    };
    let results = root.path().join("results");
    let directory = startup::run(
        settings.clone(),
        Some(app.clone()),
        results.clone(),
        None,
        worker(),
    )
    .await
    .unwrap();
    let samples: Vec<startup::Sample> =
        serde_json::from_slice(&std::fs::read(directory.join("results.json")).unwrap()).unwrap();
    assert_eq!(samples[0].measurement.as_ref().unwrap().degraded.len(), 1);
    let mut required = settings;
    required.required_mcp.push("unavailable".into());
    assert!(
        startup::run(required, Some(app), results.clone(), None, worker())
            .await
            .is_err()
    );
    let mut failed = false;
    for entry in std::fs::read_dir(results).unwrap() {
        let entry = entry.unwrap();
        let samples: Vec<startup::Sample> =
            serde_json::from_slice(&std::fs::read(entry.path().join("results.json")).unwrap())
                .unwrap();
        failed |= samples[0].status == "failed";
    }
    assert!(failed);
}
