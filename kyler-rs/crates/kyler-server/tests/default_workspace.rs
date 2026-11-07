use std::sync::Arc;

use serde_json::{Value, json};
use tokio::net::TcpListener;

use kyler_config::{AppConfig, SandboxSettings, SecretString, SkillSettings};
use kyler_core::sandbox::NetworkAccess;
use kyler_protocol::sandbox::SandboxMode;
use kyler_server::{Server, workspaces::WorkspaceFactory};

#[tokio::test]
async fn default_workspace_is_server_owned_without_authorizing_its_config_parent() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let unrelated = root.path().join("unrelated");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&unrelated).unwrap();
    let private_config = root.path().join("config.toml");
    std::fs::write(&private_config, "private-fixture").unwrap();
    let mut config = AppConfig::default();
    config.llm.api_key = SecretString::new("fixture");
    config.llm.model = "fixture".into();
    config.llm.base_url = "http://127.0.0.1:1/v1".into();
    config.session.enabled = true;
    config.session.directory = root.path().join("sessions");
    config.skills = SkillSettings {
        user_directory: root.path().join("skills"),
        state_file: root.path().join("skills.json"),
        ..Default::default()
    };
    let factory = WorkspaceFactory::configured(
        std::env::current_exe().unwrap(),
        vec![project.clone()],
        config.sandbox.clone(),
    )
    .unwrap();
    let server = Server::from_config_with_workspaces(&config, Arc::new(factory))
        .await
        .unwrap();
    let default = root.path().join("workspace").canonicalize().unwrap();
    let snapshot = server
        .state
        .runs
        .environment_for_create(None)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.workspace_root, default);
    assert!(
        server
            .state
            .runs
            .environment_for_create(unrelated.to_str())
            .is_err()
    );
    let policy_factory = WorkspaceFactory::configured(
        std::env::current_exe().unwrap(),
        vec![project.clone()],
        config.sandbox.clone(),
    )
    .unwrap()
    .with_default_workspace(&default)
    .unwrap();
    let environment = policy_factory.create(&default).unwrap();
    assert!(
        environment
            .policy()
            .filesystem
            .check_read(&default, &private_config)
            .is_err()
    );
    assert!(
        environment
            .policy()
            .filesystem
            .check_write(&default, &default.join("result.txt"))
            .is_ok()
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&private_config, default.join("config-link")).unwrap();
        assert!(
            environment
                .policy()
                .filesystem
                .check_read(&default, &default.join("config-link"))
                .is_err()
        );
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.serve(listener, async {
        let _ = stopped.await;
    }));
    let http = reqwest::Client::new();
    let info: Value = http
        .get(format!("{base}/v1/info"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        info["default_workspace"],
        default.to_string_lossy().as_ref()
    );
    let draft: Value = http
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"draft":true}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(draft["workspace_root"], info["default_workspace"]);
    let saved: Value = http
        .post(format!("{base}/v1/sessions"))
        .json(&json!({"workspace_root":project}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        saved["workspace_root"],
        project.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    let id = saved["session_id"].as_str().unwrap().to_owned();
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();

    config.workspace.default_directory = root.path().join("new-default");
    let factory = WorkspaceFactory::configured(
        std::env::current_exe().unwrap(),
        vec![project.clone()],
        config.sandbox.clone(),
    )
    .unwrap();
    let server = Server::from_config_with_workspaces(&config, Arc::new(factory))
        .await
        .unwrap();
    let saved = server.state.runs.store.load(&id).await.unwrap();
    assert_eq!(
        saved.environment.unwrap().project_root.unwrap(),
        project.canonicalize().unwrap()
    );
    assert_eq!(
        server
            .state
            .runs
            .environment_for_create(None)
            .unwrap()
            .unwrap()
            .workspace_root,
        config.workspace.default_directory.canonicalize().unwrap()
    );
}

#[test]
fn networking_defaults_do_not_widen_previously_bound_offline_sessions() {
    let root = tempfile::tempdir().unwrap();
    let worker = std::env::current_exe().unwrap();
    let offline = WorkspaceFactory::configured(
        worker.clone(),
        vec![],
        SandboxSettings {
            network_access: false,
            ..Default::default()
        },
    )
    .unwrap();
    let saved = offline
        .create(root.path())
        .unwrap()
        .session_environment()
        .clone();
    let online =
        WorkspaceFactory::configured(worker.clone(), vec![], SandboxSettings::default()).unwrap();
    let current = online.create(root.path()).unwrap();
    assert!(matches!(
        current.policy().network,
        NetworkAccess::AllowOutbound
    ));
    assert!(matches!(
        current.read_only().policy().network,
        NetworkAccess::Deny
    ));
    assert!(matches!(
        online.restore(&saved).unwrap().policy().network,
        NetworkAccess::Deny
    ));
    let readonly = WorkspaceFactory::configured(
        worker.clone(),
        vec![],
        SandboxSettings {
            mode: SandboxMode::ReadOnly,
            network_access: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(
        readonly.create(root.path()).unwrap().policy().network,
        NetworkAccess::Deny
    ));
    let extra = tempfile::tempdir().unwrap();
    let broader = WorkspaceFactory::configured(
        worker,
        vec![],
        SandboxSettings {
            additional_write_roots: vec![extra.path().to_path_buf()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        broader.restore(&saved).is_err(),
        "network compatibility must not accept changed filesystem boundaries"
    );
}
