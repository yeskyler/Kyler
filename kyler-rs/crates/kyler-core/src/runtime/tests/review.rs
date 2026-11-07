use super::{agent, server};
use crate::{
    SessionStore,
    subagent::review::tests::{fixture, git, report},
    subagent::{AgentKind, DelegateTaskArgs, SubagentManager, SubagentStatus, TaskOwner, WaitMode},
};
use kyler_protocol::{
    prompts::REVIEW,
    review::{ReviewAssessment, ReviewCoverageStatus, ReviewRequest, ReviewTarget},
};
use serde_json::json;
use std::sync::Arc;

fn request() -> DelegateTaskArgs {
    DelegateTaskArgs {
        agent: AgentKind::Review,
        review: Some(ReviewRequest {
            target: ReviewTarget::Uncommitted,
            paths: Vec::new(),
            include_untracked: true,
        }),
        task: "Review the code change. Do not fix it.".into(),
        context: Some("Check value semantics.".into()),
    }
}
fn owner() -> TaskOwner {
    TaskOwner {
        session_id: uuid::Uuid::new_v4().to_string(),
        execution_id: uuid::Uuid::new_v4().to_string(),
    }
}
#[tokio::test]
async fn review_delegation_uses_scoped_tools_and_persists_validated_findings_without_writes() {
    let (root, environment) = fixture();
    std::fs::write(root.path().join("code.rs"), "pub fn value() -> u32 { 0 }\n").unwrap();
    let finding = json!({"assessment":"issues_found","summary":"Value changed unexpectedly.","findings":[{"priority":"P1","title":"Unexpected zero return value","explanation":"Callers expecting a positive value now receive zero.","location":{"path":"code.rs","start_line":1,"end_line":1},"confidence":0.9}],"coverage":{"status":"complete","limitations":[]}});
    let (url,requests,model)=server(vec![
        json!({"role":"assistant","tool_calls":[{"id":"read","type":"function","function":{"name":"read_review_file","arguments":"{\"path\":\"code.rs\"}"}},{"id":"write","type":"function","function":{"name":"apply_patch","arguments":"{\"input\":\"fake\"}"}}]}),
        json!({"role":"assistant","content":finding.to_string()})
    ]).await;
    let storage = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::new(storage.path()).await.unwrap());
    let manager = Arc::new(
        SubagentManager::new(Default::default())
            .unwrap()
            .with_session_store(store.clone()),
    );
    let runtime = agent(url).with_environment(environment);
    let scope = manager
        .open_scope(owner(), runtime.child_executor(&manager))
        .unwrap();
    let task = scope.delegate(request(), "review-call").unwrap();
    let done = scope
        .wait(&[task.task_id], WaitMode::All, 10)
        .await
        .unwrap();
    assert!(!done.timed_out);
    let result = &done.tasks[0];
    assert_eq!(
        result.status,
        SubagentStatus::Completed,
        "{:?}",
        result.error
    );
    let review = result.review.as_ref().unwrap();
    assert_eq!(review.report.assessment, ReviewAssessment::IssuesFound);
    assert_eq!(review.report.findings.len(), 1);
    assert!(!review.snapshot.stale);
    assert!(
        review
            .report
            .coverage
            .limitations
            .iter()
            .any(|text| text.contains("tests"))
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("code.rs")).unwrap(),
        "pub fn value() -> u32 { 0 }\n"
    );
    let child = store.load(&result.session_id).await.unwrap();
    assert_eq!(child.parent.as_ref().unwrap().task_id, result.task_id);
    assert_eq!(
        child.state["kyler.review.result"],
        serde_json::to_value(review).unwrap()
    );
    std::fs::remove_file(
        storage
            .path()
            .join(&result.session_id)
            .join("snapshot.json"),
    )
    .unwrap();
    assert_eq!(
        store.load(&result.session_id).await.unwrap().state["kyler.review.result"],
        serde_json::to_value(review).unwrap()
    );
    model.await.unwrap();
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        assert!(
            request["messages"][0]["content"]
                .as_str()
                .unwrap()
                .starts_with(REVIEW.trim())
        );
        let names = request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 3);
        assert!(names.iter().all(|name| matches!(
            *name,
            "read_review_file" | "list_review_files" | "search_review_files"
        )));
    }
    assert!(
        requests[1]["messages"]
            .to_string()
            .contains("Unknown or unavailable tool")
    );
}
#[tokio::test]
async fn invalid_report_gets_exactly_one_tool_free_repair_and_never_becomes_a_clean_result() {
    let (root, environment) = fixture();
    std::fs::write(root.path().join("code.rs"), "pub fn value() -> u32 { 0 }\n").unwrap();
    let (url, requests, model) = server(vec![
        json!({"role":"assistant","content":"Looks good, probably."}),
        json!({"role":"assistant","content":"Still not JSON."}),
    ])
    .await;
    let manager = Arc::new(SubagentManager::new(Default::default()).unwrap());
    let runtime = agent(url).with_environment(environment);
    let scope = manager
        .open_scope(owner(), runtime.child_executor(&manager))
        .unwrap();
    let task = scope.delegate(request(), "review").unwrap();
    let done = scope
        .wait(&[task.task_id], WaitMode::All, 10)
        .await
        .unwrap();
    assert_eq!(done.tasks[0].status, SubagentStatus::Failed);
    assert!(done.tasks[0].review.is_none());
    assert!(
        done.tasks[0]
            .error
            .as_ref()
            .unwrap()
            .contains("one tool-free format repair")
    );
    model.await.unwrap();
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .get("tools")
            .is_none_or(|tools| tools.as_array().is_some_and(Vec::is_empty))
    );
}
#[tokio::test]
async fn valid_format_repair_retains_partial_coverage_and_separate_usage() {
    let (root, environment) = fixture();
    std::fs::write(root.path().join("code.rs"), "pub fn value() -> u32 { 0 }\n").unwrap();
    let mut partial = report();
    partial["assessment"] = "inconclusive".into();
    partial["coverage"]["status"] = "partial".into();
    partial["coverage"]["limitations"] =
        json!(["The preceding text did not establish a reliable conclusion."]);
    let (url, _, model) = server(vec![
        json!({"role":"assistant","content":"Not enough evidence."}),
        json!({"role":"assistant","content":partial.to_string()}),
    ])
    .await;
    let manager = Arc::new(SubagentManager::new(Default::default()).unwrap());
    let runtime = agent(url).with_environment(environment);
    let scope = manager
        .open_scope(owner(), runtime.child_executor(&manager))
        .unwrap();
    let task = scope.delegate(request(), "review").unwrap();
    let done = scope
        .wait(&[task.task_id], WaitMode::All, 10)
        .await
        .unwrap();
    assert_eq!(
        done.tasks[0].status,
        SubagentStatus::Completed,
        "{:?}",
        done.tasks[0].error
    );
    let review = done.tasks[0].review.as_ref().unwrap();
    assert_eq!(review.report.assessment, ReviewAssessment::Inconclusive);
    assert_eq!(review.report.coverage.status, ReviewCoverageStatus::Partial);
    assert_eq!(done.tasks[0].usage.total_tokens, 30);
    model.await.unwrap();
    assert!(!git(root.path(), &["status", "--porcelain"]).is_empty());
}

#[tokio::test]
async fn changing_the_target_during_review_makes_the_report_inconclusive() {
    use crate::sandbox::{Sandbox, SandboxRequest, WorkerResponse};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Mutate {
        path: std::path::PathBuf,
        captures: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl Sandbox for Mutate {
        async fn run(&self, request: SandboxRequest) -> anyhow::Result<WorkerResponse> {
            if request.worker_request.arguments["operation"] == "capture"
                && self.captures.fetch_add(1, Ordering::SeqCst) == 1
            {
                std::fs::write(&self.path, "pub fn value() -> u32 { 99 }\n")?;
            }
            crate::subagent::review::tests::Worker.run(request).await
        }
    }
    let (root, environment) = fixture();
    std::fs::write(root.path().join("code.rs"), "pub fn value() -> u32 { 0 }\n").unwrap();
    let canonical = root.path().canonicalize().unwrap();
    let environment = Arc::new(
        crate::ExecutionEnvironment::new(
            &canonical,
            &canonical,
            environment.policy().clone(),
            std::env::current_exe().unwrap(),
            Arc::new(Mutate {
                path: root.path().join("code.rs"),
                captures: AtomicUsize::new(0),
            }),
        )
        .unwrap(),
    );
    let (url, _, model) = server(vec![
        json!({"role":"assistant","content":report().to_string()}),
    ])
    .await;
    let manager = Arc::new(SubagentManager::new(Default::default()).unwrap());
    let runtime = agent(url).with_environment(environment);
    let scope = manager
        .open_scope(owner(), runtime.child_executor(&manager))
        .unwrap();
    let task = scope.delegate(request(), "review").unwrap();
    let done = scope
        .wait(&[task.task_id], WaitMode::All, 10)
        .await
        .unwrap();
    assert_eq!(
        done.tasks[0].status,
        SubagentStatus::Completed,
        "{:?}",
        done.tasks[0].error
    );
    let result = done.tasks[0].review.as_ref().unwrap();
    assert!(result.snapshot.stale);
    assert_eq!(result.report.assessment, ReviewAssessment::Inconclusive);
    assert_eq!(result.report.coverage.status, ReviewCoverageStatus::Partial);
    model.await.unwrap();
}
#[tokio::test]
async fn cancelling_a_review_waiting_for_the_model_persists_a_cancelled_child() {
    let (root, environment) = fixture();
    std::fs::write(root.path().join("code.rs"), "pub fn value() -> u32 { 0 }\n").unwrap();
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (start_signal, release_signal) = (started.clone(), release.clone());
    let router = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move || {
            let (started, release) = (start_signal.clone(), release_signal.clone());
            async move {
                started.add_permits(1);
                let _permit = release.acquire().await.unwrap();
                axum::Json(json!({}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let model = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let storage = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::new(storage.path()).await.unwrap());
    let manager = Arc::new(
        SubagentManager::new(Default::default())
            .unwrap()
            .with_session_store(store.clone()),
    );
    let runtime = agent(url).with_environment(environment);
    let scope = manager
        .open_scope(owner(), runtime.child_executor(&manager))
        .unwrap();
    let task = scope.delegate(request(), "review").unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let cancelled = scope.cancel(&task.task_id).await.unwrap();
    assert_eq!(cancelled.status, SubagentStatus::Cancelled);
    assert!(cancelled.review.is_none());
    let child = store.load(&task.session_id).await.unwrap();
    assert_eq!(
        child.current_execution().unwrap().status,
        crate::ExecutionStatus::Cancelled
    );
    assert!(child.state.get("kyler.review.result").is_none());
    release.add_permits(1);
    model.abort();
}
