use std::sync::Arc;

use serde_json::json;
use tokio::net::TcpListener;

use kyler_config::{AppConfig, SecretString, SkillSettings};
use kyler_core::{
    ExecutionEnvironment,
    sandbox::{Sandbox, SandboxPolicy, SandboxRequest, WorkerResponse},
};
use kyler_protocol::api::{
    session::SessionSummary,
    skills::{SkillDetails, SkillInfo, SkillStatus},
};
use kyler_server::Server;

struct UnusedSandbox;
#[async_trait::async_trait]
impl Sandbox for UnusedSandbox {
    async fn run(&self, _: SandboxRequest) -> anyhow::Result<WorkerResponse> {
        anyhow::bail!("skill management must not execute a worker")
    }
}
#[tokio::test]
async fn skill_api_requires_current_reviewed_fingerprint_and_preserves_workspace_policy() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("project");
    let skill = workspace.join(".kyler/skills/review");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: review\ndescription: Review changes\n---\nReview project instructions.",
    )
    .unwrap();
    let mut config = AppConfig::default();
    config.llm.api_key = SecretString::new("fixture");
    config.llm.base_url = "https://example.com/v1".into();
    config.llm.model = "test".into();
    config.session.enabled = true;
    config.session.directory = root.path().join("sessions");
    config.skills = SkillSettings {
        user_directory: root.path().join("user"),
        state_file: root.path().join("skills-state.json"),
        ..Default::default()
    };
    config.server.access_token = SecretString::new("server-key");
    let mut policy = SandboxPolicy::read_only("skills-test", &workspace);
    policy.process.allow_commands = false;
    let environment = Arc::new(
        ExecutionEnvironment::new(
            &workspace,
            &workspace,
            policy,
            std::env::current_exe().unwrap(),
            Arc::new(UnusedSandbox),
        )
        .unwrap(),
    );
    let server = Server::from_config_with_environment(&config, environment)
        .await
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.serve(listener, async {
        let _ = stopped.await;
    }));
    let anonymous = reqwest::Client::new();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        "Bearer server-key".parse().unwrap(),
    );
    let http = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();
    let projects_url = format!("{base}/v1/workspaces");
    let project_request =
        json!({"path":std::fs::canonicalize(&workspace).unwrap(),"title":"Empty project"});
    let empty: serde_json::Value = http
        .post(&projects_url)
        .json(&project_request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let remove = format!("{projects_url}/{}", empty["id"].as_str().unwrap());
    assert_eq!(
        anonymous.delete(&remove).send().await.unwrap().status(),
        401
    );
    assert_eq!(http.delete(&remove).send().await.unwrap().status(), 204);
    assert!(skill.join("SKILL.md").is_file());
    let remaining: serde_json::Value = http
        .get(&projects_url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(remaining.as_array().unwrap().is_empty());
    http.post(&projects_url)
        .json(&project_request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let session: SessionSummary = http
        .post(format!("{base}/v1/sessions"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        session.cwd.as_deref(),
        workspace.canonicalize().unwrap().to_str()
    );
    let url = format!("{base}/v1/sessions/{}/skills", session.session_id);
    assert_eq!(anonymous.get(&url).send().await.unwrap().status(), 401);
    let list: Vec<SkillInfo> = http.get(&url).send().await.unwrap().json().await.unwrap();
    let review = list.iter().find(|s| s.name == "review").unwrap();
    assert_eq!(review.status, SkillStatus::NeedsApproval);
    let item = format!("{url}/{}", review.id);
    let details: SkillDetails = http.get(&item).send().await.unwrap().json().await.unwrap();
    assert!(details.instructions.contains("Review project instructions"));
    assert_eq!(
        http.post(format!("{item}/enable"))
            .json(&json!({"fingerprint":"stale"}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        http.post(format!("{item}/enable"))
            .json(&json!({"fingerprint":details.info.fingerprint}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let enabled: Vec<SkillInfo> = http.get(&url).send().await.unwrap().json().await.unwrap();
    assert_eq!(
        enabled.iter().find(|s| s.name == "review").unwrap().status,
        SkillStatus::Enabled
    );
    std::fs::write(skill.join("script.sh"), "echo changed").unwrap();
    assert_eq!(
        http.post(format!("{item}/enable"))
            .json(&json!({"fingerprint":details.info.fingerprint}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let refreshed: Vec<SkillInfo> = http
        .post(format!("{url}/reload"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        refreshed
            .iter()
            .find(|s| s.name == "review")
            .unwrap()
            .status,
        SkillStatus::NeedsApproval
    );
    assert_ne!(
        details.info.fingerprint,
        refreshed
            .iter()
            .find(|s| s.name == "review")
            .unwrap()
            .fingerprint
    );
    assert_eq!(
        http.post(format!("{item}/disable"))
            .json(&json!({"fingerprint":refreshed.iter().find(|s| s.name == "review").unwrap().fingerprint}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let disabled: Vec<SkillInfo> = http.get(&url).send().await.unwrap().json().await.unwrap();
    assert_eq!(
        disabled.iter().find(|s| s.name == "review").unwrap().status,
        SkillStatus::Disabled
    );
    let user_request = json!({
        "name": "from-web", "scope": "user",
        "files": {"SKILL.md": "---\nname: from-web\ndescription: User added skill\n---\nHelp with a task."},
        "expected_fingerprint": null
    });
    assert_eq!(
        anonymous
            .post(&url)
            .json(&user_request)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let created = http.post(&url).json(&user_request).send().await.unwrap();
    assert_eq!(created.status(), 201);
    let created: SkillInfo = created.json().await.unwrap();
    assert_eq!(created.name, "from-web");
    assert!(root.path().join("user/from-web/SKILL.md").is_file());
    assert_eq!(
        http.post(&url)
            .json(&user_request)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let mut project_request = user_request.clone();
    project_request["scope"] = json!("project");
    project_request["name"] = json!("unexpected-project");
    assert_eq!(
        http.post(&url)
            .json(&project_request)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert!(!workspace.join(".kyler/skills/unexpected-project").exists());

    let connectors = format!("{base}/v1/sessions/{}/mcp", session.session_id);
    let definition = json!({"id":"user_connector", "transport":"streamable_http", "url":"http://127.0.0.1:1/mcp", "bearer_token_env": null});
    assert_eq!(
        anonymous
            .post(&connectors)
            .json(&definition)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let connector = http
        .post(&connectors)
        .json(&definition)
        .send()
        .await
        .unwrap();
    assert_eq!(connector.status(), 201);
    assert_eq!(
        connector.json::<serde_json::Value>().await.unwrap()["status"],
        "disconnected"
    );
    assert_eq!(
        http.post(&connectors)
            .json(&definition)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let mut invalid = definition.clone();
    invalid["id"] = json!("invalid");
    invalid["url"] = json!("https://user:password@example.com/mcp");
    assert_eq!(
        http.post(&connectors)
            .json(&invalid)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let catalog: serde_json::Value = http
        .get(&connectors)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(catalog[0]["status"], "disconnected");
    assert!(config.session.directory.join("_connectors.json").is_file());
    let projects = format!("{base}/v1/workspaces");
    let request = json!({"path": session.project.clone().unwrap(), "title": "My project"});
    assert_eq!(
        anonymous
            .post(&projects)
            .json(&request)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let created = http.post(&projects).json(&request).send().await.unwrap();
    assert_eq!(created.status(), 200);
    let created: serde_json::Value = created.json().await.unwrap();
    let mut renamed = request.clone();
    renamed["title"] = json!("Project title");
    let renamed: serde_json::Value = http
        .post(&projects)
        .json(&renamed)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["id"], renamed["id"]);
    let projects_list: serde_json::Value = http
        .get(&projects)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(projects_list.as_array().unwrap().len(), 1);
    assert_eq!(projects_list[0]["title"], "Project title");
    let invalid = json!({"path": root.path().join("outside"), "title":"Unauthorized"});
    assert_eq!(
        http.post(&projects)
            .json(&invalid)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let mut empty_title = request.clone();
    empty_title["title"] = json!(" ");
    assert_eq!(
        http.post(&projects)
            .json(&empty_title)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let sessions: serde_json::Value = http
        .get(format!("{base}/v1/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sessions["sessions"].as_array().unwrap().len(), 1);
    assert!(config.session.directory.join("_workspaces.json").is_file());
    let remove_nonempty = format!("{projects}/{}", created["id"].as_str().unwrap());
    assert_eq!(
        http.delete(remove_nonempty).send().await.unwrap().status(),
        409
    );
    assert!(workspace.is_dir());
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn structured_skill_selection_is_versioned_and_part_of_admission_identity() {
    use axum::{
        Json, Router,
        extract::State,
        response::sse::{Event, Sse},
        routing::post,
    };
    use kyler_protocol::api::run::{RunAccepted, RunInfo, RunStatus};
    use std::{
        convert::Infallible,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };
    async fn model(
        State(calls): State<Arc<AtomicUsize>>,
        Json(request): Json<serde_json::Value>,
    ) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
        calls.fetch_add(1, Ordering::SeqCst);
        assert!(
            request["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["role"] == "tool"
                    && message["content"]
                        .as_str()
                        .is_some_and(|text| text.contains("HTTP_SELECTED_SKILL_BODY")))
        );
        let chunk=json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":{"role":"assistant","content":"loaded"},"finish_reason":"stop"}]}).to_string();
        Sse::new(futures::stream::iter(vec![
            Ok(Event::default().data(chunk)),
            Ok(Event::default().data("[DONE]")),
        ]))
    }
    async fn terminal(http: &reqwest::Client, base: &str, id: &str) -> RunInfo {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let run: RunInfo = http
                    .get(format!("{base}/v1/runs/{id}"))
                    .send()
                    .await
                    .unwrap()
                    .error_for_status()
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                if run.status.is_terminal() {
                    return run;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
    }
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("project");
    let skill = root.path().join("user/selected");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: selected\ndescription: User-selected guidance\n---\nHTTP_SELECTED_SKILL_BODY",
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let model_base = format!("http://{}/v1", listener.local_addr().unwrap());
    let model_calls = calls.clone();
    let upstream = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/v1/chat/completions", post(model))
                .with_state(model_calls),
        )
        .await
        .unwrap()
    });
    let mut config = AppConfig::default();
    config.llm.base_url = model_base;
    config.llm.model = "fixture".into();
    config.llm.api_key = SecretString::new("fixture");
    config.skills = SkillSettings {
        user_directory: root.path().join("user"),
        state_file: root.path().join("state.json"),
        ..Default::default()
    };
    config.session.enabled = true;
    config.session.directory = root.path().join("sessions");
    let mut policy = SandboxPolicy::read_only("skill-selection", &workspace);
    policy.process.allow_commands = false;
    let environment = Arc::new(
        ExecutionEnvironment::new(
            &workspace,
            &workspace,
            policy,
            std::env::current_exe().unwrap(),
            Arc::new(UnusedSandbox),
        )
        .unwrap(),
    );
    let server = Server::from_config_with_environment(&config, environment)
        .await
        .unwrap();
    let store = server.state.runs.store.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.serve(listener, async {
        let _ = stopped.await;
    }));
    let http = reqwest::Client::new();
    let session: SessionSummary = http
        .post(format!("{base}/v1/sessions"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let list: Vec<SkillInfo> = http
        .get(format!("{base}/v1/sessions/{}/skills", session.session_id))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let info = list.iter().find(|info| info.name == "selected").unwrap();
    let reference = json!({"id":info.id,"fingerprint":info.fingerprint});
    let url = format!("{base}/v1/sessions/{}/runs", session.session_id);
    assert_eq!(http.post(&url).json(&json!({"input":"invalid","skills":[{"id":"/tmp/untrusted/SKILL.md","fingerprint":"0".repeat(64)}]})).send().await.unwrap().status(),400);
    let request = json!({"input":"Follow the selected skill","skills":[reference.clone()],"client_request_id":"selected-op"});
    let accepted: RunAccepted = http
        .post(&url)
        .json(&request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let replay: RunAccepted = http
        .post(&url)
        .json(&request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(accepted.run_id, replay.run_id);
    let mut conflict = request.clone();
    conflict["skills"][0]["fingerprint"] = json!("0".repeat(64));
    assert_eq!(
        http.post(&url)
            .json(&conflict)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let finished = terminal(&http, &base, &accepted.run_id).await;
    assert_eq!(
        finished.status,
        RunStatus::Completed,
        "{:?}",
        finished.error
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let saved = store.load(&session.session_id).await.unwrap();
    assert_eq!(
        saved.current_execution().unwrap().selected_skills[0].fingerprint,
        info.fingerprint
    );
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: selected\ndescription: Changed\n---\nNew version",
    )
    .unwrap();
    let stale:RunAccepted=http.post(&url).json(&json!({"input":"Use the old reference","skills":[reference],"client_request_id":"stale-op"})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(
        terminal(&http, &base, &stale.run_id).await.status,
        RunStatus::Failed
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
    upstream.abort();
}
