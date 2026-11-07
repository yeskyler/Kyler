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
            .json(&json!({"workspace_root":project}))
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
    assert_eq!(first.workspace_root, second.workspace_root);
    assert_ne!(first.working_directory, second.working_directory);
    let a = store.load(&first.session_id).await.unwrap();
    let b = store.load(&second.session_id).await.unwrap();
    let environment = factory.restore(a.environment.as_ref().unwrap()).unwrap();
    let other = b.environment.as_ref().unwrap().workspace_root.clone();
    let own = environment.session_environment().workspace_root.clone();
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
        !permissions["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|option| option["mode"] == "full_access" && option["available"] == true)
    );
    // Legacy sessions retain their records but are rebound before a new execution.
    let mut legacy = Session::new();
    legacy.environment = Some(
        factory
            .create(&project)
            .unwrap()
            .session_environment()
            .clone(),
    );
    store.save(&mut legacy).await.unwrap();
    std::fs::write(project.join("owned.txt"), "legacy-owned-output").unwrap();
    kyler_core::artifacts::ArtifactStore::new(config.session.directory.join("_artifacts"))
        .capture(
            &legacy.session_id,
            &uuid::Uuid::new_v4().to_string(),
            "[Owned](owned.txt)",
            Some(&factory.create(&project).unwrap()),
        )
        .unwrap();
    let admitted:Value=client.post(format!("{base}/v1/sessions/{}/runs",legacy.session_id)).json(&json!({"input":"fixture connection failure","client_request_id":"isolation-migration"})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    for _ in 0..100 {
        let run: Value = client
            .get(format!(
                "{base}/v1/runs/{}",
                admitted["run_id"].as_str().unwrap()
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if matches!(
            run["status"].as_str(),
            Some("failed" | "completed" | "cancelled")
        ) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let migrated = store.load(&legacy.session_id).await.unwrap();
    let migrated = migrated.environment.unwrap();
    assert_eq!(migrated.project_root, Some(project.canonicalize().unwrap()));
    assert_ne!(migrated.workspace_root, project);
    assert!(!migrated.workspace_root.join("public.txt").exists());
    assert_eq!(
        std::fs::read_to_string(migrated.workspace_root.join("owned.txt")).unwrap(),
        "legacy-owned-output"
    );
    assert!(project.join("public.txt").exists());
    // A missing private workspace is not silently replaced by its public project.
    let missing = create(&client, &base, &project).await;
    let path = store
        .load(&missing.session_id)
        .await
        .unwrap()
        .environment
        .unwrap()
        .workspace_root;
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
