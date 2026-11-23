mod support;
use axum::{
    Json, Router,
    extract::State,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::post,
};
use kyler_core::{
    Agent, ExecutionEnvironment, SandboxPolicy, ToolBox,
    sandbox::{Sandbox, SandboxRequest, WorkerResponse},
    subagent::SubagentManager,
};
use kyler_protocol::{
    api::run::{RunAccepted, RunStatus},
    review::{AgentKind, ReviewAssessment},
};
use serde_json::{Value, json};
use std::{
    process::Command,
    sync::{Arc, Mutex},
};
use support::Harness;

struct Worker;
#[async_trait::async_trait]
impl Sandbox for Worker {
    async fn run(&self, request: SandboxRequest) -> anyhow::Result<WorkerResponse> {
        assert!(!request.worker_request.allow_commands);
        Ok(kyler_core::sandbox::worker::dispatch(request.worker_request).await)
    }
}
async fn completion(
    State(requests): State<Arc<Mutex<Vec<Value>>>>,
    Json(request): Json<Value>,
) -> Response {
    requests.lock().unwrap().push(request.clone());
    if request["stream"] != true {
        let names = request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 3);
        assert!(!names.contains(&"run_command") && !names.contains(&"ask_question"));
        let report = json!({"assessment":"no_findings","summary":"No actionable defect found.","findings":[],"coverage":{"status":"complete","limitations":[]}});
        return Json(json!({"id":"child","object":"chat.completion","created":0,"model":"test","choices":[{"index":0,"message":{"role":"assistant","content":report.to_string()},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}})).into_response();
    }
    let results = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| serde_json::from_str::<Value>(message["content"].as_str().unwrap()).unwrap())
        .collect::<Vec<_>>();
    let (delta, reason) = if request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .is_some_and(|message| {
            message["content"]
                .as_str()
                .is_some_and(|text| text.contains("No Git change set is available"))
        }) {
        (
            json!({"content":"The current workspace is not a Git repository. Please provide a repository path, commit ID, or diff."}),
            "stop",
        )
    } else if let Some(task) = results
        .iter()
        .filter_map(|result| result["tasks"].as_array())
        .flatten()
        .find(|task| task["status"] == "completed")
    {
        assert_eq!(task["agent"], "review");
        assert_eq!(task["review"]["report"]["assessment"], "no_findings");
        (
            json!({"content":"Review completed. No actionable findings; static inspection only."}),
            "stop",
        )
    } else if let Some(task) = results.iter().find(|result| result["task_id"].is_string()) {
        (
            json!({"tool_calls":[{"index":0,"id":format!("wait-{}",results.len()),"function":{"name":"wait_tasks","arguments":json!({"task_ids":[task["task_id"]],"mode":"all","timeout_secs":5}).to_string()}}]}),
            "tool_calls",
        )
    } else {
        (
            json!({"tool_calls":[{"index":0,"id":"review-call","function":{"name":"delegate_task","arguments":json!({"agent":"review","task":"Review the current code change","review":{"target":{"type":"uncommitted"},"paths":["code.rs"]}}).to_string()}}]}),
            "tool_calls",
        )
    };
    let body=json!({"id":"parent","object":"chat.completion.chunk","created":0,"model":"test","choices":[{"index":0,"delta":delta,"finish_reason":reason}]}).to_string();
    Sse::new(futures::stream::iter(vec![
        Ok::<_, std::convert::Infallible>(Event::default().data(body)),
        Ok(Event::default().data("[DONE]")),
    ]))
    .into_response()
}
#[tokio::test]
async fn review_is_delegated_collected_and_persisted_through_the_server() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Review fixture"],
        vec!["config", "user.email", "review@example.invalid"],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(root.join("code.rs"), "pub fn value() -> u32 { 1 }\n").unwrap();
    assert!(
        Command::new("git")
            .args(["add", "code.rs"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "base"
            ])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(root.join("code.rs"), "pub fn value() -> u32 { 2 }\n").unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/v1/chat/completions", post(completion))
        .with_state(requests.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let model = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let mut harness = Harness::new(Default::default()).await;
    harness.close().await;
    let environment = Arc::new(
        ExecutionEnvironment::new(
            &root,
            &root,
            SandboxPolicy::workspace_write("review-server", &root),
            std::env::current_exe().unwrap(),
            Arc::new(Worker),
        )
        .unwrap(),
    );
    let manager = Arc::new(
        SubagentManager::new(Default::default())
            .unwrap()
            .with_session_store(harness.store.clone()),
    );
    harness.agent = Arc::new(
        Agent::new(
            url,
            "fixture".into(),
            "test",
            None::<String>,
            Arc::new(ToolBox::new()),
        )
        .with_environment(environment)
        .with_subagent_manager(manager),
    );
    harness.restart().await;
    let session = harness.session().await;
    let private = harness
        .store
        .load(&session)
        .await
        .unwrap()
        .environment
        .unwrap()
        .cwd;
    // A project-bound session already executes in the seeded repository.
    if private.canonicalize().unwrap() != root.canonicalize().unwrap() {
        assert!(
            Command::new("git")
                .args(["clone", "--quiet", "--no-hardlinks", "--local"])
                .arg(&root)
                .arg(&private)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(private.join("code.rs"), "pub fn value() -> u32 { 2 }\n").unwrap();
    let url = format!("{}/v1/sessions/{session}/runs", harness.url);
    let command =
        json!({"input": "", "command": {"type": "review", "target": {"type": "uncommitted"}}});
    let metadata = private.join(".git");
    let parked_metadata = private.join("fixture-git-metadata");
    std::fs::rename(&metadata, &parked_metadata).unwrap();
    let response = harness
        .client
        .post(&url)
        .json(&command)
        .send()
        .await
        .unwrap();
    let fallback: RunAccepted = response.error_for_status().unwrap().json().await.unwrap();
    let result = harness.terminal(&fallback.run_id).await;
    std::fs::rename(&parked_metadata, &metadata).unwrap();
    assert_eq!(result.status, RunStatus::Completed, "{:?}", result.error);
    assert!(
        result
            .output
            .unwrap()
            .contains("Please provide a repository path, commit ID, or diff.")
    );
    assert!(
        harness
            .store
            .load(&session)
            .await
            .unwrap()
            .subagent_tasks
            .is_empty()
    );

    let run: RunAccepted = harness
        .client
        .post(url)
        .json(&command)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let result = harness.terminal(&run.run_id).await;
    assert_eq!(result.status, RunStatus::Completed, "{:?}", result.error);
    assert!(result.output.unwrap().contains("static inspection only"));
    let saved = harness.store.load(&session).await.unwrap();
    assert_eq!(saved.subagent_tasks.len(), 1);
    let task = saved.subagent_tasks.values().next().unwrap();
    assert_eq!(task.agent, AgentKind::Review);
    assert_eq!(
        task.review.as_ref().unwrap().report.assessment,
        ReviewAssessment::NoFindings
    );
    let child = harness.store.load(&task.session_id).await.unwrap();
    assert!(child.state.contains_key("kyler.review.result"));
    harness.restart().await;
    assert!(
        harness
            .store
            .load(&session)
            .await
            .unwrap()
            .subagent_tasks
            .values()
            .all(|task| task.review.is_some())
    );
    assert_eq!(
        std::fs::read_to_string(root.join("code.rs")).unwrap(),
        "pub fn value() -> u32 { 2 }\n"
    );
    assert_eq!(requests.lock().unwrap().len(), 5);
    harness.close().await;
    model.abort();
}
