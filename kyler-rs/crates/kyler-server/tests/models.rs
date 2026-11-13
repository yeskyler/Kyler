use std::{convert::Infallible, path::Path, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::State,
    response::sse::{Event, Sse},
    routing::post,
};
use serde_json::{Value, json};
use tokio::{
    net::TcpListener,
    sync::{Mutex, oneshot, watch},
};

use kyler_config::{SecretString, loader};
use kyler_protocol::api::{
    models::{ModelCatalog, ModelConnectionSettings, ModelConnectionUpdate, SessionModel},
    run::{RunAccepted, RunInfo},
};
use kyler_server::Server;

#[derive(Clone)]
struct Fixture {
    requests: Arc<Mutex<Vec<Value>>>,
    release: watch::Sender<bool>,
    authorization: Arc<Mutex<Vec<String>>>,
}
async fn completion(
    State(fixture): State<Fixture>,
    headers: axum::http::HeaderMap,
    Json(request): Json<Value>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    fixture.authorization.lock().await.push(
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned(),
    );
    fixture.requests.lock().await.push(request.clone());
    Sse::new(async_stream::stream! {
        if request["model"] == "slow-v1" {
            let mut release = fixture.release.subscribe();
            release.wait_for(|v| *v).await.unwrap();
        }
        let model = request["model"].as_str().unwrap();
        yield Ok(Event::default().data(json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":model,"choices":[{"index":0,"delta":{"role":"assistant","content":model},"finish_reason":"stop"}]}).to_string()));
        yield Ok(Event::default().data(json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":model,"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}).to_string()));
        yield Ok(Event::default().data("[DONE]"));
    })
}
fn config(url: &str, sessions: &Path) -> String {
    format!(
        r#"
[session]
enabled = true
directory = "{}"
[skills]
enabled = false
[models]
default = "fast"
[models.providers.fixture]
base_url = "{url}"
api_key = "PRIVATE_MODEL_KEY"
[models.profiles.fast]
# Keep this 32k fixture independent of system-prompt byte growth.
capabilities = {{ tokenizer = "cl100k_base" }}
provider = "fixture"
model = "fast-v1"
enable_thinking = true
context_tokens = 32000
output_tokens = 1024
[models.profiles.slow]
provider = "fixture"
model = "slow-v1"
context_tokens = 64000
output_tokens = 2048
output_token_parameter = "max_tokens"
"#,
        sessions.display()
    )
}
async fn terminal(client: &reqwest::Client, url: &str, run: &str) -> RunInfo {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let info: RunInfo = client
                .get(format!("{url}/v1/runs/{run}"))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            if info.status.is_terminal() {
                return info;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn model_selection_reload_and_streaming_runs_keep_independent_snapshots() {
    let root = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture = Fixture {
        requests: Default::default(),
        release: watch::channel(false).0,
        authorization: Default::default(),
    };
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/v1/chat/completions", post(completion))
        .with_state(fixture.clone());
    let model = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let source = root.path().join("config.toml");
    let text = config(&base, &root.path().join("sessions"));
    std::fs::write(&source, &text).unwrap();
    let mut config = loader::load_from(&source).unwrap();
    config.server.access_token = SecretString::new("fixture-access");
    let server = Server::from_config(&config).await.unwrap();
    server
        .state
        .runs
        .set_model_source(source.clone(), Default::default());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stop, done) = oneshot::channel();
    let http = tokio::spawn(async move {
        server
            .serve(listener, async {
                let _ = done.await;
            })
            .await
            .unwrap()
    });
    let client = reqwest::Client::builder()
        .default_headers(
            [(
                reqwest::header::AUTHORIZATION,
                "Bearer fixture-access".parse().unwrap(),
            )]
            .into_iter()
            .collect(),
        )
        .build()
        .unwrap();
    assert_eq!(
        reqwest::Client::new()
            .post(format!("{url}/v1/models/reload"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let session: Value = client
        .post(format!("{url}/v1/sessions"))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = session["session_id"].as_str().unwrap();
    let selection = format!("{url}/v1/sessions/{id}/model");
    let thinking_url = format!("{url}/v1/sessions/{id}/thinking");
    std::fs::write(
        config
            .session
            .directory
            .join("_runs")
            .join(format!("{id}.thinking")),
        "false",
    )
    .unwrap();
    assert_eq!(
        reqwest::Client::new()
            .get(&thinking_url)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let thinking: Value = client
        .get(&thinking_url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        thinking,
        json!({"enabled":true,"supported":true,"level":"high","levels":["high","max"]})
    );
    assert_eq!(
        client
            .put(&thinking_url)
            .json(&json!({"enabled":false}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        client
            .put(&thinking_url)
            .json(&json!({"level":"max"}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    client
        .put(&thinking_url)
        .json(&json!({"level":"high"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        client
            .put(&thinking_url)
            .json(&json!({"level":"off"}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        client
            .put(&thinking_url)
            .json(&json!({"level":"invalid"}))
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    assert_eq!(
        client
            .put(&thinking_url)
            .json(&json!({"level":"high","enabled":true}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );

    let other: Value = client
        .post(format!("{url}/v1/sessions"))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_id = other["session_id"].as_str().unwrap();
    let other_selection = format!("{url}/v1/sessions/{other_id}/model");
    let other_thinking: Value = client
        .get(format!("{url}/v1/sessions/{other_id}/thinking"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(other_thinking["enabled"], true);

    client
        .put(&other_selection)
        .json(&json!({"profile":"slow"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        client
            .put(format!("{url}/v1/sessions/{other_id}/thinking"))
            .json(&json!({"level":"high"}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .put(&selection)
            .json(&json!({"profile":"fast", "base_url":"http://invalid"}))
            .send()
            .await
            .unwrap()
            .status(),
        422
    );

    client
        .put(&selection)
        .json(&json!({"profile":"slow"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let run: RunAccepted = client
        .post(format!("{url}/v1/sessions/{id}/runs"))
        .json(&json!({"input":"hello"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.requests.lock().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let switched: SessionModel = client
        .put(&selection)
        .json(&json!({"profile":"fast"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(switched.active.as_ref().unwrap().model, "slow-v1");
    assert_eq!(switched.selected.as_ref().unwrap().model, "fast-v1");
    std::fs::write(&source, "invalid configuration PRIVATE_BROKEN_KEY").unwrap();
    assert_eq!(
        client
            .post(format!("{url}/v1/models/reload"))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let catalog: ModelCatalog = client
        .get(format!("{url}/v1/models"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(catalog.revision, 1);
    let replacement = text
        .split("[models.profiles.slow]")
        .next()
        .unwrap()
        .replace("fast-v1", "fast-v2");
    std::fs::write(&source, replacement).unwrap();
    let catalog: ModelCatalog = client
        .post(format!("{url}/v1/models/reload"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(catalog.revision, 2);
    assert_eq!(catalog.models.len(), 1);
    let unavailable: SessionModel = client
        .get(&other_selection)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(unavailable.selected.is_none() && unavailable.error.is_some());
    assert_eq!(
        client
            .post(format!("{url}/v1/sessions/{other_id}/runs"))
            .json(&json!({"input":"must not fall back"}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );

    fixture.release.send_replace(true);
    let completed = terminal(&client, &url, &run.run_id).await;
    assert_eq!(completed.output.as_deref(), Some("slow-v1"));
    assert_eq!(completed.model.unwrap().revision, 1);
    let next: RunAccepted = client
        .post(format!("{url}/v1/sessions/{id}/runs"))
        .json(&json!({"input":"next"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let completed = terminal(&client, &url, &next.run_id).await;
    assert_eq!(completed.output.as_deref(), Some("fast-v2"));
    assert_eq!(completed.model.unwrap().context_window_tokens, 32000);
    client
        .put(&thinking_url)
        .json(&json!({"enabled":true}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let thinking_run: RunAccepted = client
        .post(format!("{url}/v1/sessions/{id}/runs"))
        .json(&json!({"input":"thinking on"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        terminal(&client, &url, &thinking_run.run_id)
            .await
            .output
            .as_deref(),
        Some("fast-v2")
    );
    client
        .put(&thinking_url)
        .json(&json!({"level":"high"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let requests = fixture.requests.lock().await;
    assert_eq!(requests[0]["max_tokens"], 2048);
    assert!(requests[0].get("max_completion_tokens").is_none());
    assert_eq!(requests[1]["max_completion_tokens"], 1024);
    assert_eq!(requests[1]["enable_thinking"], true);
    assert_eq!(requests[1]["reasoning_effort"], "high");
    assert_eq!(requests[2]["enable_thinking"], true);
    assert!(requests[1].get("max_tokens").is_none());
    drop(requests);
    let id = id.to_owned();
    stop.send(()).unwrap();
    http.await.unwrap();
    // A separately stored next-run preference survives server restart without changing run checkpoints.
    let server = Server::from_config(&config).await.unwrap();
    let selection = server.state.runs.session_model(&id, None).await.unwrap();
    assert_eq!(selection.profile.as_deref(), Some("fast"));
    assert!(selection.active.is_none());
    assert!(
        server
            .state
            .runs
            .session_thinking(&id, None)
            .await
            .unwrap()
            .enabled
    );
    let session = server.state.runs.store.load(&id).await.unwrap();
    assert_eq!(
        session.executions[0].model_snapshot.as_ref().unwrap().model,
        "slow-v1"
    );
    assert_eq!(
        session
            .execution
            .as_ref()
            .unwrap()
            .model_snapshot
            .as_ref()
            .unwrap()
            .model,
        "fast-v2"
    );
    assert!(
        !serde_json::to_string(&session)
            .unwrap()
            .contains("PRIVATE_MODEL_KEY")
    );
    server.state.runs.delete_session(&id).await.unwrap();
    assert!(
        !config
            .session
            .directory
            .join("_runs")
            .join(format!("{id}.thinking"))
            .exists()
    );
    assert!(
        !config
            .session
            .directory
            .join("_runs")
            .join(format!("{id}.model"))
            .exists()
    );
    server.state.runs.shutdown().await;
    model.abort();
}

#[tokio::test]
async fn editable_model_connection_is_private_durable_and_next_turn_only() {
    let root = tempfile::tempdir().unwrap();
    let mut models = Vec::new();
    let mut endpoints = Vec::new();
    let mut fixtures = Vec::new();
    for _ in 0..2 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        endpoints.push(format!("http://{}/v1", listener.local_addr().unwrap()));
        let fixture = Fixture {
            requests: Default::default(),
            release: watch::channel(false).0,
            authorization: Default::default(),
        };
        fixtures.push(fixture.clone());
        let router = Router::new()
            .route("/v1/chat/completions", post(completion))
            .with_state(fixture);
        models.push(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap()
        }));
    }
    let source = root.path().join("config.toml");
    std::fs::write(
        &source,
        config(&endpoints[0], &root.path().join("sessions")),
    )
    .unwrap();
    let mut settings = loader::load_from(&source).unwrap();
    settings.server.access_token = SecretString::new("fixture-access");
    let server = Server::from_config(&settings).await.unwrap();
    server
        .state
        .runs
        .set_model_source(source.clone(), Default::default());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(server.serve(listener, async {
        let _ = stopped.await;
    }));
    let client = reqwest::Client::builder()
        .default_headers(
            [(
                reqwest::header::AUTHORIZATION,
                "Bearer fixture-access".parse().unwrap(),
            )]
            .into_iter()
            .collect(),
        )
        .build()
        .unwrap();
    let session: Value = client
        .post(format!("{url}/v1/sessions"))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = session["session_id"].as_str().unwrap();
    let selection = format!("{url}/v1/sessions/{id}/model");
    let connection = format!("{selection}/settings");
    client
        .put(&selection)
        .json(&json!({"profile":"slow"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        reqwest::Client::new()
            .get(&connection)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        reqwest::Client::new()
            .put(&connection)
            .json(&json!({"url":endpoints[1],"model":"new","key":"PRIVATE_WEB_KEY"}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let visible = client
        .get(&connection)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!visible.contains("PRIVATE_MODEL_KEY"));
    let visible: ModelConnectionSettings = serde_json::from_str(&visible).unwrap();
    assert_eq!(visible.url, endpoints[0]);
    assert!(visible.key_configured);
    let active: RunAccepted = client
        .post(format!("{url}/v1/sessions/{id}/runs"))
        .json(&json!({"input":"hello"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixtures[0].requests.lock().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let saved = client.put(&connection).json(&json!({"url":format!("{}/chat/completions/",endpoints[1]),"model":"new-model","key":"PRIVATE_WEB_KEY"})).send().await.unwrap().error_for_status().unwrap().text().await.unwrap();
    assert!(!saved.contains("PRIVATE_WEB_KEY"));
    let saved: ModelConnectionSettings = serde_json::from_str(&saved).unwrap();
    assert_eq!(saved.url, endpoints[1]);
    let state: SessionModel = client
        .get(&selection)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state.active.unwrap().model, "slow-v1");
    assert_eq!(state.selected.unwrap().model, "new-model");
    fixtures[0].release.send_replace(true);
    assert_eq!(
        terminal(&client, &url, &active.run_id)
            .await
            .output
            .as_deref(),
        Some("slow-v1")
    );
    let next: RunAccepted = client
        .post(format!("{url}/v1/sessions/{id}/runs"))
        .json(&json!({"input":"next"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        terminal(&client, &url, &next.run_id)
            .await
            .output
            .as_deref(),
        Some("new-model")
    );
    assert_eq!(
        fixtures[0].authorization.lock().await[0],
        "Bearer PRIVATE_MODEL_KEY"
    );
    assert_eq!(
        fixtures[1].authorization.lock().await[0],
        "Bearer PRIVATE_WEB_KEY"
    );
    let changed = loader::load_from(&source).unwrap();
    let effective = changed.effective_models();
    let fast = &effective.profiles["fast"];
    assert_eq!(effective.providers[&fast.provider].base_url, endpoints[0]);
    assert_eq!(
        effective.providers[&fast.provider].api_key.expose(),
        "PRIVATE_MODEL_KEY"
    );
    let changed_text = std::fs::read_to_string(&source).unwrap();
    for bad_url in [
        "file:///private/data",
        "https://user:password@example.com/v1",
        "https://example.com/v1?key=PRIVATE_REJECTED_KEY",
    ] {
        let response = client
            .put(&connection)
            .json(&json!({"url":bad_url,"model":"invalid","key":"PRIVATE_REJECTED_KEY"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert!(
            !response
                .text()
                .await
                .unwrap()
                .contains("PRIVATE_REJECTED_KEY")
        );
    }
    assert_eq!(std::fs::read_to_string(&source).unwrap(), changed_text);
    let saved: ModelConnectionSettings = client
        .put(&connection)
        .json(&json!({"url":endpoints[1],"model":"retained-key","key":""}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(saved.key_configured);
    let retained = loader::load_from(&source).unwrap().effective_models();
    assert_eq!(
        retained.providers[&retained.profiles["slow"].provider]
            .api_key
            .expose(),
        "PRIVATE_WEB_KEY"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&source).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let catalog = client
        .post(format!("{url}/v1/models/reload"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!catalog.contains("PRIVATE_WEB_KEY"));
    assert!(catalog.contains("retained-key"));
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
    for model in models {
        model.abort();
    }
}

#[tokio::test]
async fn legacy_connection_edit_preserves_other_config_and_respects_startup_overrides() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("config.toml");
    let text = format!(
        "[llm]\nbase_url=\"http://127.0.0.1:1/v1\"\nmodel=\"old\"\napi_key=\"PRIVATE_LEGACY_KEY\"\n[session]\nenabled=true\ndirectory={:?}\n[skills]\nenabled=false\n[server]\nmax_concurrent_runs=3\n",
        root.path().join("sessions").to_str().unwrap()
    );
    std::fs::write(&source, text).unwrap();
    let config = loader::load_from(&source).unwrap();
    let server = Server::from_config(&config).await.unwrap();
    server
        .state
        .runs
        .set_model_source(source.clone(), Default::default());
    assert_eq!(
        server
            .state
            .runs
            .global_model_connection()
            .await
            .unwrap()
            .model,
        "old"
    );
    server
        .state
        .runs
        .update_global_model_connection(ModelConnectionUpdate {
            url: "http://127.0.0.1:1/v1".into(),
            model: "global-model".into(),
            key: None,
        })
        .await
        .unwrap();
    assert!(server.state.runs.store.list().await.unwrap().is_empty());
    let session = server.state.runs.store.create().await.unwrap();
    let result = server
        .state
        .runs
        .update_model_connection(
            &session.session_id,
            ModelConnectionUpdate {
                url: "https://example.com/v1".into(),
                model: "replacement".into(),
                key: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(result.model, "replacement");
    let loaded = loader::load_from(&source).unwrap();
    assert_eq!(loaded.llm.api_key.expose(), "PRIVATE_LEGACY_KEY");
    assert_eq!(loaded.server.max_concurrent_runs, 3);
    let before = std::fs::read_to_string(&source).unwrap();
    server.state.runs.set_model_source(
        source.clone(),
        loader::LlmOverrides {
            model: Some("fixed".into()),
            ..Default::default()
        },
    );
    let error = server
        .state
        .runs
        .update_model_connection(
            &session.session_id,
            ModelConnectionUpdate {
                url: "https://other.example/v1".into(),
                model: "blocked".into(),
                key: Some("NEW_PRIVATE_KEY".into()),
            },
        )
        .await
        .unwrap_err();
    assert!(!error.body.message.contains("NEW_PRIVATE_KEY"));
    assert!(error.body.message.contains("override"));
    assert_eq!(std::fs::read_to_string(source).unwrap(), before);
    assert_eq!(
        server.state.runs.model_catalog().unwrap().models[0].model,
        "replacement"
    );
    server.state.runs.shutdown().await;
}
