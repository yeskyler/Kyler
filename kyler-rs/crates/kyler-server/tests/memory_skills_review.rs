//! Real authenticated HTTP review, isolated workspaces, and no model requests.
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::net::TcpListener;

use kyler_config::{AppConfig, SecretString, SkillSettings};
use kyler_core::{
    memory::{MemoryStatus, MemoryStore},
    skills::SkillManager,
};
use kyler_protocol::api::{
    session::SessionSummary,
    skills::{SaveSkillRequest, SkillScope},
};
use kyler_server::{Server, workspaces::WorkspaceFactory};

#[tokio::test]
async fn review_is_authenticated_scoped_and_rejects_stale_memory_and_draft_versions() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let mut config = AppConfig::default();
    config.llm.api_key = SecretString::new("fixture");
    config.llm.base_url = "http://127.0.0.1:1/v1".into();
    config.llm.model = "fixture".into();
    config.session.enabled = true;
    config.session.directory = root.path().join("sessions");
    config.server.access_token = SecretString::new("fixture-token");
    config.skills = SkillSettings {
        user_directory: root.path().join("skills"),
        state_file: root.path().join("skills-state.json"),
        ..Default::default()
    };
    let factory = Arc::new(
        WorkspaceFactory::native(
            std::env::current_exe().unwrap(),
            vec![project.clone()],
            false,
            false,
            false,
        )
        .unwrap(),
    );
    let server = Server::from_config_with_workspaces(&config, factory)
        .await
        .unwrap();
    let store = server.state.runs.store.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.serve(listener, async {
        let _ = stopped.await;
    }));
    let http = reqwest::Client::builder()
        .no_proxy()
        .tls_certs_only([])
        .build()
        .unwrap();
    async fn create(
        http: &reqwest::Client,
        base: &str,
        project: &std::path::Path,
    ) -> SessionSummary {
        http.post(format!("{base}/v1/sessions"))
            .bearer_auth("fixture-token")
            .json(&json!({"cwd":project,"workspace_mode":"isolated"}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    let a = create(&http, &base, &project).await;
    let b = create(&http, &base, &project).await;
    let scope_a = MemoryStore::scope(&store.load(&a.session_id).await.unwrap());
    let scope_b = MemoryStore::scope(&store.load(&b.session_id).await.unwrap());
    assert_ne!(scope_a, scope_b);
    let memory = MemoryStore::new(store.directory().join("_memory"));
    let key = "workflow/with spaces";
    memory
        .propose(
            &scope_a,
            key,
            "Run targeted checks",
            vec![],
            Default::default(),
            None,
            &a.session_id,
            "execution",
            "call",
        )
        .unwrap();
    let prefix_a = format!("{base}/v1/sessions/{}", a.session_id);
    let prefix_b = format!("{base}/v1/sessions/{}", b.session_id);
    assert_eq!(
        http.get(format!("{prefix_a}/memory/entries"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let page: Value = http
        .get(format!("{prefix_a}/memory/entries"))
        .bearer_auth("fixture-token")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page["entries"][0]["key"], key);
    let other: Value = http
        .get(format!("{prefix_b}/memory/entries"))
        .bearer_auth("fixture-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(other["total"], 0);
    assert_eq!(
        http.get(format!("{prefix_b}/memory/entry"))
            .query(&[("key", key)])
            .bearer_auth("fixture-token")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        http.get(format!("{prefix_a}/learning/memories"))
            .bearer_auth("fixture-token")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let review = |scope: &str, revision| json!({"key":key,"expected_scope":scope,"expected_revision":revision,"status":"active"});
    assert_eq!(
        http.post(format!("{prefix_b}/memory/review"))
            .bearer_auth("fixture-token")
            .json(&review(&scope_a, 1))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(
        http.post(format!("{prefix_b}/memory/review"))
            .bearer_auth("fixture-token")
            .json(&review(&scope_b, 1))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        http.post(format!("{prefix_a}/memory/review"))
            .bearer_auth("fixture-token")
            .json(&review(&scope_a, 0))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let detail: Value = http
        .get(format!("{prefix_a}/memory/entry"))
        .query(&[("key", key)])
        .bearer_auth("fixture-token")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["entry"]["revision"], 1);
    http.post(format!("{prefix_a}/memory/review"))
        .bearer_auth("fixture-token")
        .json(&review(&scope_a, 1))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        memory.list(&scope_a).unwrap()[0].status,
        MemoryStatus::Active
    );
    assert_eq!(
        http.get(format!("{prefix_a}/memory/entries?limit=0"))
            .bearer_auth("fixture-token")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );

    let manager = SkillManager::new(config.skills.clone()).unwrap();
    let request = |body: &str, expected| SaveSkillRequest {
        name: "workflow".into(),
        scope: SkillScope::User,
        files: [
            (
                "SKILL.md".into(),
                format!("---\nname: workflow\ndescription: Reviewable workflow\n---\n{body}"),
            ),
            (
                "references/learning-source.json".into(),
                json!({"sources":[{"scope":scope_a,"key":key,"revision":2}]}).to_string(),
            ),
        ]
        .into(),
        expected_fingerprint: expected,
    };
    let draft = manager
        .save_draft(request("Inspect all files", None))
        .await
        .unwrap();
    let drafts: Value = http
        .get(format!("{prefix_a}/skills/drafts"))
        .bearer_auth("fixture-token")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(drafts["total"], 1);
    let foreign: Value = http
        .get(format!("{prefix_b}/skills/drafts"))
        .bearer_auth("fixture-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(foreign["total"], 0);
    assert_eq!(
        http.get(format!("{prefix_b}/skills/drafts/workflow"))
            .bearer_auth("fixture-token")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        http.post(format!("{prefix_b}/skills/drafts/workflow/publish"))
            .bearer_auth("fixture-token")
            .json(&json!({"expected_scope":scope_b,"fingerprint":draft.fingerprint}))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let inspected: Value = http
        .get(format!("{prefix_a}/skills/drafts/workflow"))
        .bearer_auth("fixture-token")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        inspected["draft"]["files"]["SKILL.md"],
        request("Inspect all files", None).files["SKILL.md"]
    );
    let revised = manager
        .save_draft(request(
            "Revised after review",
            Some(draft.fingerprint.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(
        http.post(format!("{prefix_a}/skills/drafts/workflow/publish"))
            .bearer_auth("fixture-token")
            .json(&json!({"expected_scope":scope_a,"fingerprint":draft.fingerprint}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    http.post(format!("{prefix_a}/skills/drafts/workflow/publish"))
        .bearer_auth("fixture-token")
        .json(&json!({"expected_scope":scope_a,"fingerprint":revised.fingerprint}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let installed: Value = http
        .get(format!("{prefix_a}/skills/drafts/workflow"))
        .bearer_auth("fixture-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(installed["installed_fingerprint"], revised.fingerprint);
    assert_eq!(http.post(format!("{prefix_a}/skills/drafts/workflow/publish")).bearer_auth("fixture-token").json(&json!({"expected_scope":scope_a,"fingerprint":revised.fingerprint,"expected_installed":"stale"})).send().await.unwrap().status(),409);
    assert!(
        manager
            .settings
            .user_directory
            .join("workflow/SKILL.md")
            .is_file()
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}
