use std::{path::PathBuf, sync::Arc};

use serde_json::{Value, json};
use tokio::net::TcpListener;

use kyler_config::{AppConfig, SecretString, SkillSettings};
use kyler_core::Session;
use kyler_protocol::api::session::SessionSummary;
use kyler_server::{Server, workspaces::WorkspaceFactory};

#[tokio::test]
async fn sessions_share_only_project_identity_and_commands_cannot_escape_their_private_root() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("public.txt"), "project content").unwrap();
    let worker =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/kyler-tool-worker");
    assert!(
        worker.is_file(),
        "Build kyler-tool-worker before the session isolation test"
    );
    let mut config = AppConfig::default();
    config.llm.api_key = SecretString::new("fixture");
    config.llm.base_url = "http://127.0.0.1:1/v1".into();
    config.llm.model = "fixture".into();
    config.session.enabled = true;
    config.session.directory = root.path().join("sessions");
    config.skills = SkillSettings {
        user_directory: root.path().join("skills"),
        state_file: root.path().join("skills-state.json"),
        ..Default::default()
    };
    let factory = Arc::new(
        WorkspaceFactory::native(worker, vec![project.clone()], true, true, false).unwrap(),
    );
    let server = Server::from_config_with_workspaces(&config, factory.clone())
        .await
        .unwrap();
    let store = server.state.runs.store.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.serve(listener, async {
        let _ = stopped.await;
    }));
    let client = reqwest::Client::new();
    async fn create(
        client: &reqwest::Client,
        base: &str,
        project: &std::path::Path,
    ) -> SessionSummary {
        client
            .post(format!("{base}/v1/sessions"))
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
    let first = create(&client, &base, &project).await;
    let second = create(&client, &base, &project).await;
    assert_eq!(first.project, second.project);
    assert_ne!(first.cwd, second.cwd);
    let a = store.load(&first.session_id).await.unwrap();
    let b = store.load(&second.session_id).await.unwrap();
    let environment = factory.restore(a.environment.as_ref().unwrap()).unwrap();
    let other = b.environment.as_ref().unwrap().cwd.clone();
    let own = environment.session_environment().cwd.clone();
    std::fs::write(own.join("own.txt"), "session-a").unwrap();
    std::fs::write(other.join("other.txt"), "session-b").unwrap();
    for path in [
        &project.join("public.txt"),
        &other.join("other.txt"),
        store.directory(),
    ] {
        assert!(
            environment
                .policy()
                .filesystem
                .check_read(&own, path)
                .is_err()
        );
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(other.join("other.txt"), own.join("escape.txt")).unwrap();
        assert!(
            environment
                .run_tool(
                    "read_file",
                    &json!({"file_path":own.join("escape.txt")}).to_string()
                )
                .await
                .is_err()
        );
    }
    assert!(
        environment
            .run_tool(
                "read_file",
                &json!({"file_path":own.join("own.txt")}).to_string()
            )
            .await
            .unwrap()
            .contains("session-a")
    );
    assert!(
        environment
            .run_tool(
                "read_file",
                &json!({"file_path":other.join("other.txt")}).to_string()
            )
            .await
            .is_err()
    );
    let command = environment
        .run_tool(
            "run_command",
            &json!({"program":"/bin/cat","args":[other.join("other.txt")]}).to_string(),
        )
        .await
        .unwrap();
    let command: Value = serde_json::from_str(&command).unwrap();
    assert_ne!(command["exit_code"], 0);
    assert!(
        !command["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("session-b")
    );
    let permissions: Value = client
        .get(format!(
            "{base}/v1/sessions/{}/permissions",
            first.session_id
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        permissions["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|option| option["mode"] == "full_access" && option["available"] == true)
    );
    // Full access is an explicit capability; selecting it requires confirmation.
    let endpoint = format!("{base}/v1/sessions/{}/permissions", first.session_id);
    assert!(
        client
            .put(&endpoint)
            .json(&json!({"mode":"full_access"}))
            .send()
            .await
            .unwrap()
            .status()
            .is_client_error()
    );
    let selected: Value = client
        .put(&endpoint)
        .json(&json!({"mode":"full_access","confirm_full_access":true}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(selected["selected"]["mode"], "full_access");
    let unchanged = store.load(&first.session_id).await.unwrap();
    assert_eq!(unchanged.environment, a.environment);
    // A missing private workspace is not silently replaced by its public project.
    let missing = create(&client, &base, &project).await;
    let path = store
        .load(&missing.session_id)
        .await
        .unwrap()
        .environment
        .unwrap()
        .cwd;
    std::fs::remove_dir(&path).unwrap();
    assert!(
        !client
            .get(format!(
                "{base}/v1/sessions/{}/permissions",
                missing.session_id
            ))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert!(!path.exists());
    // A forged owner path is rejected even if it points to a valid isolated sibling.
    let mut forged = Session::new();
    forged.environment = b.environment.clone();
    store.save(&mut forged).await.unwrap();
    assert!(
        !client
            .get(format!(
                "{base}/v1/sessions/{}/permissions",
                forged.session_id
            ))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    client
        .delete(format!("{base}/v1/sessions/{}", first.session_id))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert!(!own.exists());
    assert!(other.join("other.txt").exists());
    let _ = stop.send(());
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn project_sessions_execute_in_project_with_independent_history_and_host_boundaries() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("source.txt"), "original project file").unwrap();
    let worker =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/kyler-tool-worker");
    let mut config = AppConfig::default();
    config.llm.api_key = SecretString::new("fixture");
    config.llm.model = "fixture".into();
    config.llm.base_url = "http://127.0.0.1:1/v1".into();
    config.session.enabled = true;
    config.session.directory = root.path().join("sessions");
    config.workspace.default_directory = project.clone();
    config.skills = SkillSettings {
        enabled: false,
        ..Default::default()
    };
    let factory = Arc::new(
        WorkspaceFactory::native(worker, vec![project.clone()], true, true, false).unwrap(),
    );
    let server = Server::from_config_with_workspaces(&config, factory.clone())
        .await
        .unwrap();
    let store = server.state.runs.store.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.serve(listener, async {
        let _ = stopped.await;
    }));
    let client = reqwest::Client::new();
    let mut sessions = Vec::new();
    for index in 0..2 {
        let summary: SessionSummary = client
            .post(format!("{base}/v1/sessions"))
            .json(&if index == 0 {
                json!({"cwd":project})
            } else {
                json!({"draft":true})
            })
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            summary.cwd.as_deref(),
            project.canonicalize().unwrap().to_str()
        );
        sessions.push(summary);
    }
    assert_ne!(sessions[0].session_id, sessions[1].session_id);
    let mut first = store.load(&sessions[0].session_id).await.unwrap();
    let snapshot = first.environment.clone().unwrap();
    assert!(snapshot.managed.is_none());
    assert!(
        !store
            .directory()
            .join("_execution")
            .join(&first.session_id)
            .exists()
    );
    first
        .begin_execution("Private first-session question")
        .unwrap();
    store.save(&mut first).await.unwrap();
    assert!(
        store
            .load(&sessions[1].session_id)
            .await
            .unwrap()
            .events
            .is_empty()
    );
    let restored = factory
        .restore(
            &store
                .load(&first.session_id)
                .await
                .unwrap()
                .environment
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        restored.session_environment().cwd,
        project.canonicalize().unwrap()
    );
    let read = restored
        .run_tool("read_file", &json!({"file_path":"source.txt"}).to_string())
        .await
        .unwrap();
    assert!(read.contains("original project file"));
    let command = restored
        .run_tool(
            "run_command",
            &json!({"program":"/bin/sh","args":["-c","printf changed > source.txt; pwd"]})
                .to_string(),
        )
        .await
        .unwrap();
    assert!(command.contains(project.canonicalize().unwrap().to_str().unwrap()));
    assert_eq!(
        std::fs::read_to_string(project.join("source.txt")).unwrap(),
        "changed"
    );
    assert!(
        restored
            .policy()
            .filesystem
            .check_read(&project, store.directory())
            .is_err()
    );
    let denied = client
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"cwd":root.path()}))
        .send()
        .await
        .unwrap();
    assert!(denied.status().is_client_error());
    let conflicting = client
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"cwd":project,"workspace_root":root.path()}))
        .send()
        .await
        .unwrap();
    assert!(conflicting.status().is_client_error());
    let deleted = client
        .delete(format!("{base}/v1/sessions/{}", sessions[1].session_id))
        .send()
        .await
        .unwrap();
    assert!(deleted.status().is_success());
    assert_eq!(
        std::fs::read_to_string(project.join("source.txt")).unwrap(),
        "changed"
    );
    let mut forged = snapshot;
    forged.cwd = root.path().canonicalize().unwrap();
    assert!(factory.restore(&forged).is_err());
    let _ = stop.send(());
    task.await.unwrap().unwrap();
}
