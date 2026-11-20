use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    response::sse::{Event, Sse},
    routing::post,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, sync::oneshot};

use kyler_config::{AppConfig, SecretString};
use kyler_core::{
    ExecutionEnvironment,
    sandbox::{Sandbox, SandboxPolicy, SandboxRequest, WorkerResponse},
};
use kyler_server::Server;

struct Backend;
#[async_trait::async_trait]
impl Sandbox for Backend {
    async fn run(&self, _: SandboxRequest) -> anyhow::Result<WorkerResponse> {
        panic!("This test never executes sandbox tools")
    }
}
async fn completion(
    State(calls): State<Arc<AtomicUsize>>,
    Json(_): Json<Value>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    calls.fetch_add(1, Ordering::SeqCst);
    Sse::new(futures::stream::iter(vec![Ok(Event::default().data(json!({"id":"fixture","object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":{"role":"assistant","content":"Scheduled review complete"},"finish_reason":"stop"}]}).to_string())),Ok(Event::default().data("[DONE]"))]))
}
#[tokio::test]
async fn tasks_use_core_admission_once_and_persist_history() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("http://{}/v1", listener.local_addr().unwrap());
    let model = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(completion))
                .with_state(calls.clone()),
        )
        .into_future(),
    );
    let mut config = AppConfig::default();
    config.skills.enabled = false;
    config.session.enabled = true;
    config.session.directory = workspace.join("sessions");
    config.llm.base_url = upstream;
    config.llm.model = "fixture".into();
    config.llm.discover_models = false;
    config.llm.reasoning_effort = Some(kyler_protocol::api::models::ThinkingLevel::High);
    config.llm.api_key = SecretString::new("fixture");
    config.server.access_token = SecretString::new("test-token");
    let environment = Arc::new(
        ExecutionEnvironment::new(
            &workspace,
            &workspace,
            SandboxPolicy::workspace_write("fixture", &workspace),
            std::env::current_exe().unwrap(),
            Arc::new(Backend),
        )
        .unwrap(),
    );
    let server = Server::from_config_with_environment(&config, environment.clone())
        .await
        .unwrap();
    let store = server.state.runs.store.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let (stop, done) = oneshot::channel();
    let http = tokio::spawn(server.serve(listener, async {
        let _ = done.await;
    }));
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{base}/automations"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let request = json!({"config":{"name":"Review","prompt":"Review the project","cwd":workspace,"workspace_mode":"project","cron":"0 9 * * *","timezone":"Asia/Shanghai","model_profile":null,"thinking":"high","permission":"workspace_access","timeout_seconds":60,"wait_timeout_seconds":30},"enabled":false,"request_id":uuid::Uuid::new_v4().to_string()});
    let mut full_request = request.clone();
    full_request["config"]["permission"] = json!("full_access");
    assert_eq!(
        client
            .post(format!("{base}/automations"))
            .bearer_auth("test-token")
            .json(&full_request)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let mut invalid_root = request.clone();
    invalid_root["config"]["cwd"] = json!("/unavailable-automation-workspace");
    assert_eq!(
        client
            .post(format!("{base}/automations"))
            .bearer_auth("test-token")
            .json(&invalid_root)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let response = client
        .post(format!("{base}/automations"))
        .bearer_auth("test-token")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        201,
        "{}",
        response.text().await.unwrap_or_default()
    );
    // Re-send the identical creation request; it must return the original task.
    let task: Value = client
        .post(format!("{base}/automations"))
        .bearer_auth("test-token")
        .json(&request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = task["id"].as_str().unwrap();
    let list: Value = client
        .get(format!("{base}/automations"))
        .bearer_auth("test-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let preview: Value = client
        .post(format!("{base}/automations/preview"))
        .bearer_auth("test-token")
        .json(&json!({"cron":"0 9 * * *","timezone":"Asia/Shanghai"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preview["occurrences"].as_array().unwrap().len(), 5);
    let trigger = json!({"request_id":uuid::Uuid::new_v4().to_string()});
    let run: Value = client
        .post(format!("{base}/automations/{id}/runs"))
        .bearer_auth("test-token")
        .json(&trigger)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let again: Value = client
        .post(format!("{base}/automations/{id}/runs"))
        .bearer_auth("test-token")
        .json(&trigger)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(run["id"], again["id"]);
    let finished = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let page: Value = client
                .get(format!("{base}/automations/{id}/runs"))
                .bearer_auth("test-token")
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let run = page["runs"][0].clone();
            if ["completed", "failed", "interrupted"].contains(&run["status"].as_str().unwrap()) {
                break run;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(finished["status"], "completed", "{finished}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(finished["core_run_id"].is_string());
    let session = store
        .load(run["session_id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(session.state["automation"]["task_id"], id);
    stop.send(()).unwrap();
    http.await.unwrap().unwrap();
    drop(store);
    let restarted = Server::from_config_with_environment(&config, environment)
        .await
        .unwrap();
    let history = restarted
        .state
        .scheduler
        .as_ref()
        .unwrap()
        .store
        .runs(id, None, 20)
        .await
        .unwrap();
    assert_eq!(history.runs.len(), 1);
    assert_eq!(
        history.runs[0].status,
        kyler_protocol::api::automations::ScheduledRunStatus::Completed
    );
    model.abort();
}
