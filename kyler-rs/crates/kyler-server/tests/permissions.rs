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
use kyler_protocol::{
    api::{
        permissions::{ApprovalRecord, ApprovalStatus, PermissionMode, PermissionState},
        run::{RunAccepted, RunInfo, RunStatus},
        session::SessionSummary,
    },
    approval::ToolRule,
};
use kyler_server::Server;

struct Backend(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl Sandbox for Backend {
    async fn run(&self, request: SandboxRequest) -> anyhow::Result<WorkerResponse> {
        assert_eq!(request.worker_request.tool, "apply_patch");
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(WorkerResponse {
            output: Some("patch applied".into()),
            error: None,
        })
    }
}
async fn completion(
    Json(request): Json<Value>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let messages = request["messages"].as_array().unwrap();
    let user = messages.iter().rposition(|m| m["role"] == "user").unwrap();
    let has_result = messages[user + 1..].iter().any(|m| m["role"] == "tool");
    let patch = if messages[user]["content"] == "control" {
        "*** Begin Patch\n*** Add File: .session-store/forged\n+forged\n*** End Patch"
    } else if messages[user]["content"] == "review" {
        "*** Begin Patch\n*** Delete File: review-target.txt\n*** End Patch"
    } else {
        "*** Begin Patch\n*** Add File: result.txt\n+approved\n*** End Patch"
    };
    let (delta, reason) = if has_result {
        (json!({"role":"assistant","content":"finished"}), "stop")
    } else {
        (
            json!({"role":"assistant","tool_calls":[{"index":0,"id":"write-call","type":"function","function":{"name":"apply_patch","arguments":if messages[user]["content"] == "escalate" { json!({"input":patch,"__kyler_approval":{"request":true,"reason":"Write one file with user approval"}}).to_string() } else { json!({"input":patch}).to_string() }}}]}),
            "tool_calls",
        )
    };
    Sse::new(futures::stream::iter(vec![Ok(Event::default().data(json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":delta,"finish_reason":reason}]}).to_string())), Ok(Event::default().data("[DONE]"))]))
}
async fn terminal(http: &reqwest::Client, base: &str, id: &str) -> RunInfo {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let run: RunInfo = http
                .get(format!("{base}/v1/runs/{id}"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if run.status.is_terminal() {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
async fn pending(http: &reqwest::Client, base: &str, session: &str) -> ApprovalRecord {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let records: Vec<ApprovalRecord> = http
                .get(format!("{base}/v1/sessions/{session}/approvals"))
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
                .find(|r| r.status == ApprovalStatus::Pending)
            {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
async fn start(http: &reqwest::Client, base: &str, session: &str) -> RunAccepted {
    start_input(http, base, session, "write the file").await
}
async fn start_input(
    http: &reqwest::Client,
    base: &str,
    session: &str,
    input: &str,
) -> RunAccepted {
    http.post(format!("{base}/v1/sessions/{session}/runs"))
        .json(&json!({"input":input}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}
#[tokio::test]
async fn approval_gates_side_effects_and_permissions_update_during_run() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("http://{}/v1", listener.local_addr().unwrap());
    let model = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/v1/chat/completions", post(completion)),
        )
        .await
        .unwrap()
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let mut policy = SandboxPolicy::workspace_write("fixture", &workspace);
    policy.process.allow_commands = true;
    let environment = Arc::new(
        ExecutionEnvironment::new(
            &workspace,
            &workspace,
            policy,
            std::env::current_exe().unwrap(),
            Arc::new(Backend(calls.clone())),
        )
        .unwrap(),
    );
    let mut config = AppConfig::default();
    config.llm.base_url = upstream;
    config.llm.model = "fixture".into();
    config.llm.api_key = SecretString::new("fixture");
    config.skills.enabled = false;
    config
        .approval
        .rules
        .insert("apply_patch".into(), ToolRule::Ask);
    config.session.enabled = true;
    config.session.directory = workspace.join(".session-store");
    config.server.run_timeout_seconds = 1;
    config.server.approval_timeout_seconds = 5;
    let server = Server::from_config_with_environment(&config, environment)
        .await
        .unwrap();
    let store = server.state.runs.store.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, done) = oneshot::channel();
    let task = tokio::spawn(server.serve(listener, async {
        let _ = done.await;
    }));
    let http = reqwest::Client::new();
    let session: SessionSummary = http
        .post(format!("{base}/v1/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    std::fs::write(
        std::path::Path::new(session.working_directory.as_ref().unwrap()).join("review-target.txt"),
        "review target",
    )
    .unwrap();
    let id = &session.session_id;
    let permissions = format!("{base}/v1/sessions/{id}/permissions");
    let state: PermissionState = http
        .get(&permissions)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state.selected.mode, PermissionMode::WorkspaceAccess);
    assert_eq!(
        state.approval.reviewer,
        kyler_protocol::approval::ApprovalReviewer::User
    );
    assert_eq!(state.options.len(), 3);
    assert!(
        !state
            .options
            .iter()
            .find(|o| o.mode == PermissionMode::FullAccess)
            .unwrap()
            .available
    );
    assert_eq!(
        http.put(&permissions)
            .json(&json!({"mode":"full_access"}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    http.put(&permissions)
        .json(&json!({"mode":"workspace_access"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let run = start(&http, &base, id).await;
    let request = pending(&http, &base, id).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // Human waits use a separate budget; they must not exhaust the one-second run limit.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let active: RunInfo = http
        .get(format!("{base}/v1/runs/{}", run.run_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!active.status.is_terminal());
    let saved = store.load(id).await.unwrap();
    assert_eq!(
        saved.current_execution().unwrap().approvals[0].status,
        ApprovalStatus::Pending
    );
    let switched: PermissionState = http
        .put(&permissions)
        .json(&json!({"mode":"workspace_access", "approval":{"policy":"never","reviewer":"user","rules":{"apply_patch":"allow"}}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        switched.active.unwrap().mode,
        PermissionMode::WorkspaceAccess
    );
    assert_eq!(switched.selected.mode, PermissionMode::WorkspaceAccess);
    let completed = terminal(&http, &base, &run.run_id).await;
    assert_eq!(completed.status, RunStatus::Completed);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // The old approval cannot authorize an operation after a policy revision.
    assert_eq!(
        http.post(format!("{base}/v1/approvals/{}/decision", request.id))
            .json(&json!({"decision":"allow_once","digest":request.digest}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let run = start(&http, &base, id).await;
    terminal(&http, &base, &run.run_id).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let control = start_input(&http, &base, id, "control").await;
    terminal(&http, &base, &control.run_id).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "Approval never cannot modify server permission storage inside the workspace"
    );

    http.put(&permissions)
        .json(&json!({"mode":"auto_review"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let run = start_input(&http, &base, id, "review").await;
    let request = pending(&http, &base, id).await;
    http.post(format!("{base}/v1/approvals/{}/decision", request.id))
        .json(&json!({"decision":"allow_session","digest":request.digest}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    terminal(&http, &base, &run.run_id).await;
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let run = start_input(&http, &base, id, "review").await;
    terminal(&http, &base, &run.run_id).await;
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    // A new policy revision invalidates prior session grants, even for identical arguments.
    http.put(&permissions)
        .json(&json!({"mode":"auto_review"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let run = start_input(&http, &base, id, "review").await;
    let request = pending(&http, &base, id).await;
    http.post(format!("{base}/v1/runs/{}/cancel", run.run_id))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        terminal(&http, &base, &run.run_id).await.status,
        RunStatus::Cancelled
    );
    assert_eq!(
        http.post(format!("{base}/v1/approvals/{}/decision", request.id))
            .json(&json!({"decision":"allow_once","digest":request.digest}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    // Tightening the policy also re-reviews an already waiting operation.
    let run = start_input(&http, &base, id, "review").await;
    let old_request = pending(&http, &base, id).await;
    http.put(&permissions)
        .json(&json!({"mode":"read_only"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    terminal(&http, &base, &run.run_id).await;
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        http.post(format!("{base}/v1/approvals/{}/decision", old_request.id))
            .json(&json!({"decision":"allow_once","digest":old_request.digest}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let kept: PermissionState = http
        .put(&permissions)
        .json(&json!({"approval":{"policy":"never","reviewer":"user"}}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(kept.selected.mode, PermissionMode::ReadOnly);
    assert_eq!(
        kept.sandbox,
        Some(kyler_protocol::sandbox::SandboxMode::ReadOnly)
    );
    let blocked = start(&http, &base, id).await;
    terminal(&http, &base, &blocked.run_id).await;
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    // An explicit legacy sandbox-preset change is distinct from changing approvals.
    http.put(&permissions)
        .json(&json!({"mode":"auto_review"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let state: PermissionState = http.put(&permissions)
        .json(&json!({"approval":{"policy":"on-request","reviewer":"user","rules":{"apply_patch":"ask"}}})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(
        state.sandbox,
        Some(kyler_protocol::sandbox::SandboxMode::WorkspaceWrite)
    );
    let run = start(&http, &base, id).await;
    let old = pending(&http, &base, id).await;
    let state: PermissionState = http
        .put(&permissions)
        .json(
            &json!({"approval":{"policy":"never","reviewer":"user","rules":{"apply_patch":"ask"}}}),
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        state.approval.policy,
        kyler_protocol::approval::ApprovalPolicy::Never
    );
    terminal(&http, &base, &run.run_id).await;
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        http.post(format!("{base}/v1/approvals/{}/decision", old.id))
            .json(&json!({"decision":"allow_once","digest":old.digest}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    http.put(&permissions)
        .json(&json!({"approval":{"policy":"never","rules":{"apply_patch":"allow"}}}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let run = start(&http, &base, id).await;
    terminal(&http, &base, &run.run_id).await;
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    let control = start_input(&http, &base, id, "control").await;
    terminal(&http, &base, &control.run_id).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        5,
        "allow rules and never cannot bypass the sandbox"
    );
    // Read-only remains the session preset after a single explicitly requested write.
    http.put(&permissions)
        .json(&json!({"mode":"read_only"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let run = start_input(&http, &base, id, "escalate").await;
    let exception = pending(&http, &base, id).await;
    assert!(exception.reason.contains("only to this call"));
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    http.post(format!("{base}/v1/approvals/{}/decision", exception.id))
        .json(&json!({"decision":"allow_once","digest":exception.digest}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    terminal(&http, &base, &run.run_id).await;
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    let state: PermissionState = http
        .get(&permissions)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state.selected.mode, PermissionMode::ReadOnly);
    let blocked = start(&http, &base, id).await;
    terminal(&http, &base, &blocked.run_id).await;
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    assert_eq!(
        http.put(&permissions)
            .json(&json!({"mode":"full_access","confirm_full_access":true}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let saved = store.load(id).await.unwrap();
    assert!(saved.executions.iter().any(|e| {
        e.approvals
            .iter()
            .any(|r| r.status == ApprovalStatus::Denied)
    }));
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
    model.abort();
}
