use std::{
    collections::{BTreeMap, HashMap},
    future::IntoFuture,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{net::TcpListener, process::Command};

use kyler_config::{McpServerSettings, McpSettings, McpTransport};
use kyler_core::{
    ExecutionContext, ExecutionEnvironment, Tool,
    mcp::McpManager,
    sandbox::SandboxPolicy,
    tools::{ToolContext, ToolOutputBudget, ToolSource},
};
use kyler_protocol::api::mcp::{McpAction, McpStatus};

async fn call(tool: &dyn Tool) -> anyhow::Result<String> {
    let mut execution = ExecutionContext::new();
    let mut state = HashMap::new();
    tool.execute(
        "{}",
        &mut ToolContext {
            processes: None,
            execution: &mut execution,
            state: &mut state,
            session_id: "session",
            tool_call_id: "call",
            subagents: None,
            environment: None,
        },
    )
    .await
}
async fn wait(
    manager: &McpManager,
    session: &str,
    env: Option<Arc<ExecutionEnvironment>>,
    target: McpStatus,
) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if manager.list(session, env.clone()).await.unwrap()[0].status == target {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn http_hot_connect_error_results_disconnect_reconnect_and_typed_output() {
    use axum::{
        Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post,
    };

    let calls = Arc::new(AtomicUsize::new(0));
    async fn handler(
        State(calls): State<Arc<AtomicUsize>>,
        Json(request): Json<Value>,
    ) -> axum::response::Response {
        if request.get("id").is_none() {
            return StatusCode::ACCEPTED.into_response();
        }
        let result = match request["method"].as_str().unwrap() {
            "initialize" => {
                json!({"protocolVersion":request["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}})
            }
            "tools/list" => {
                json!({"tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object"}}]})
            }
            "tools/call" => {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                json!({"content":[{"type":"text","text":"hello"}],"structuredContent":{"value":42},"isError":n==1})
            }
            _ => json!({}),
        };
        Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result})).into_response()
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/mcp", post(handler))
                .with_state(calls.clone()),
        )
        .into_future(),
    );
    let settings = McpSettings {
        servers: BTreeMap::from([(
            "fixture".into(),
            McpServerSettings {
                transport: McpTransport::StreamableHttp,
                url,
                auto_connect: false,
                ..Default::default()
            },
        )]),
    };
    let system_root = tempfile::tempdir().unwrap();
    let manager = McpManager::new(settings)
        .unwrap()
        .with_user_store(system_root.path().join("connectors.json"))
        .unwrap();
    assert_eq!(
        manager.list("first", None).await.unwrap()[0].status,
        McpStatus::Disconnected
    );
    manager
        .action("first", None, "fixture", McpAction::Connect)
        .await
        .unwrap();
    wait(&manager, "first", None, McpStatus::Connected).await;
    let tools = manager.snapshot("first", None).await.unwrap();
    let tool = &tools["mcp__fixture__echo"];
    assert!(matches!(tool.source(),ToolSource::Mcp {server_id,..} if server_id=="fixture"));
    let result = call(tool.as_ref()).await.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result).unwrap()["structuredContent"]["value"],
        42
    );
    assert!(call(tool.as_ref()).await.is_err());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "error calls must not be retried"
    );
    let compact = tool
        .compress_output(&result.repeat(100), &ToolOutputBudget { max_bytes: 256 })
        .unwrap();
    assert!(compact.content.len() <= 256);
    assert_eq!(
        serde_json::from_str::<Value>(&compact.content).unwrap()["truncated"],
        true
    );
    manager
        .action("first", None, "fixture", McpAction::Disconnect)
        .await
        .unwrap();
    wait(&manager, "first", None, McpStatus::Disconnected).await;
    assert!(manager.snapshot("first", None).await.unwrap().is_empty());
    assert!(
        call(tool.as_ref()).await.is_err(),
        "old snapshot cannot run after disconnect"
    );
    assert_eq!(
        manager.list("second", None).await.unwrap()[0].status,
        McpStatus::Disconnected
    );
    manager
        .action("first", None, "fixture", McpAction::Reconnect)
        .await
        .unwrap();
    wait(&manager, "first", None, McpStatus::Connected).await;
    assert_eq!(manager.snapshot("first", None).await.unwrap().len(), 1);
    assert!(
        manager
            .action("first", None, "unknown", McpAction::Connect)
            .await
            .is_err()
    );
    // System discovery requires neither a workspace nor a user session.
    manager
        .system_action("fixture", McpAction::Connect)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if manager.system_list().await.unwrap()[0].status == McpStatus::Connected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        manager.snapshot("new-project", None).await.unwrap().len(),
        1
    );
    let policy = SandboxPolicy::read_only("no-network", system_root.path());
    let environment = Arc::new(
        ExecutionEnvironment::mxc(
            system_root.path(),
            system_root.path(),
            policy,
            std::env::current_exe().unwrap(),
        )
        .unwrap(),
    );
    assert!(
        manager
            .snapshot("denied-project", Some(environment))
            .await
            .unwrap()
            .is_empty(),
        "system connection never bypasses a session network policy"
    );
    manager
        .system_action("fixture", McpAction::Disconnect)
        .await
        .unwrap();
    assert!(
        manager
            .snapshot("new-project", None)
            .await
            .unwrap()
            .is_empty()
    );
    manager.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn native_stdio_uses_isolated_workspace_and_cleans_up_on_disconnect() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("server.py");
    std::fs::write(&script, include_str!("fixtures/mcp_server.py")).unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    let workspace = root.path().canonicalize().unwrap();
    let mut policy = SandboxPolicy::read_only("mcp-test", &workspace);
    if cfg!(target_os = "macos") {
        policy
            .runtime_readonly_paths
            .push("/Library/Developer/CommandLineTools".into());
    }
    let env = Arc::new(
        ExecutionEnvironment::mxc(
            &workspace,
            &workspace,
            policy,
            env!("CARGO_BIN_EXE_kyler-tool-worker"),
        )
        .unwrap(),
    );
    let settings = McpSettings {
        servers: BTreeMap::from([(
            "local".into(),
            McpServerSettings {
                command: "/usr/bin/python3".into(),
                args: vec![
                    "-u".into(),
                    script.to_string_lossy().into_owned(),
                    outside.path().to_string_lossy().into_owned(),
                ],
                ..Default::default()
            },
        )]),
    };
    let manager = McpManager::new(settings).unwrap();
    let tools = manager
        .snapshot("session", Some(env.clone()))
        .await
        .unwrap();
    assert_eq!(
        tools.len(),
        1,
        "{:?}",
        manager.list("session", Some(env.clone())).await.unwrap()
    );
    let result: Value =
        serde_json::from_str(&call(tools["mcp__local__inspect"].as_ref()).await.unwrap()).unwrap();
    assert_eq!(
        result["structuredContent"]["cwd"],
        workspace.to_string_lossy().as_ref()
    );
    assert_eq!(result["structuredContent"]["outside_readable"], false);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if manager.list("session", Some(env.clone())).await.unwrap()[0]
                .tools
                .len()
                == 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(tools.len(), 1, "in-flight snapshot remains immutable");
    assert_eq!(
        manager
            .snapshot("session", Some(env.clone()))
            .await
            .unwrap()
            .len(),
        2
    );
    let pid = result["structuredContent"]["pid"].as_i64().unwrap();
    manager
        .action("session", Some(env.clone()), "local", McpAction::Disconnect)
        .await
        .unwrap();
    wait(&manager, "session", Some(env), McpStatus::Disconnected).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .stderr(Stdio::null())
                .status()
                .await
                .unwrap();
            if !status.success() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    manager.shutdown().await;
}

#[test]
fn mcp_credentials_must_be_resolved_by_configuration_not_ambient_environment() {
    let mut settings = McpSettings {
        servers: BTreeMap::from([(
            "secured".into(),
            McpServerSettings {
                transport: McpTransport::StreamableHttp,
                url: "https://example.com/mcp".into(),
                bearer_token_env: Some("PATH".into()),
                ..Default::default()
            },
        )]),
    };
    // PATH is normally present, but the manager must never read it implicitly.
    assert!(McpManager::new(settings.clone()).is_err());
    settings.resolve_environment(&HashMap::from([(
        "PATH".into(),
        "explicit-test-token".into(),
    )]));
    assert!(McpManager::new(settings).is_ok());
}

#[tokio::test]
async fn user_connectors_are_durable_private_and_do_not_connect_until_requested() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("connectors.json");
    let manager = McpManager::new(McpSettings::default())
        .unwrap()
        .with_user_store(path.clone())
        .unwrap();
    let definition = McpServerSettings {
        transport: McpTransport::StreamableHttp,
        url: "http://127.0.0.1:1/mcp".into(),
        resolved_token: Some(kyler_config::SecretString::new("must-not-be-persisted")),
        ..Default::default()
    };
    let info = manager
        .add_user_connector("user_server".into(), definition.clone())
        .await
        .unwrap();
    assert_eq!(info.status, McpStatus::Disconnected);
    assert_eq!(
        manager.list("first", None).await.unwrap()[0].status,
        McpStatus::Disconnected
    );
    assert_eq!(
        manager.list("second", None).await.unwrap()[0].id,
        "user_server"
    );
    let bytes = std::fs::read_to_string(&path).unwrap();
    assert!(!bytes.contains("must-not-be-persisted"));
    assert!(!bytes.contains("resolved_token"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(
        manager
            .add_user_connector("user_server".into(), definition.clone())
            .await
            .is_err()
    );
    assert!(
        manager
            .add_user_connector("bad-name".into(), definition.clone())
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
    let reloaded = McpManager::new(McpSettings::default())
        .unwrap()
        .with_user_store(path.clone())
        .unwrap();
    assert_eq!(
        reloaded.list("third", None).await.unwrap()[0].status,
        McpStatus::Disconnected
    );
    // Existing session catalogs gain new definitions without reconnecting old slots.
    manager
        .add_user_connector("another".into(), definition)
        .await
        .unwrap();
    assert_eq!(manager.list("first", None).await.unwrap().len(), 2);
    let protected = McpManager::new(McpSettings {
        servers: BTreeMap::from([(
            "user_server".into(),
            McpServerSettings {
                transport: McpTransport::StreamableHttp,
                url: "http://127.0.0.1:1/operator".into(),
                ..Default::default()
            },
        )]),
    })
    .unwrap()
    .with_user_store(path.clone())
    .unwrap();
    assert!(
        protected
            .remove_user_connector("user_server")
            .await
            .is_err()
    );
    protected.shutdown().await;
    manager.remove_user_connector("user_server").await.unwrap();
    assert_eq!(manager.list("first", None).await.unwrap().len(), 1);
    assert_eq!(manager.list("second", None).await.unwrap()[0].id, "another");
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("user_server")
    );
    let after = McpManager::new(McpSettings::default())
        .unwrap()
        .with_user_store(path)
        .unwrap();
    assert_eq!(after.configured_servers().unwrap().len(), 1);
    assert!(after.remove_user_connector("user_server").await.is_err());
    after.shutdown().await;
    manager.shutdown().await;
    reloaded.shutdown().await;
}

#[test]
fn connector_mentions_are_explicit_validated_and_deduplicated() {
    assert_eq!(
        kyler_core::mcp::references(
            "@connector:api_1 @connector:api_1 @skill-creator @connector:other @connector:../bad @connector:"
        ),
        vec!["api_1", "other"]
    );
}

fn connector_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, content) in files {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(content).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[tokio::test]
async fn packages_are_bounded_persistent_and_never_execute_during_install() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("connectors.json");
    let manager = McpManager::new(McpSettings::default())
        .unwrap()
        .with_user_store(store.clone())
        .unwrap();
    let bytes = connector_zip(&[
        (
            "connector.json",
            br#"{"id":"package_fixture","command":"server","args":["${package}/data.txt"]}"#,
        ),
        ("server", b"#!/bin/sh\nexit 42\n"),
        ("data.txt", b"resource"),
    ]);
    let info = manager.install_package(bytes.clone()).await.unwrap();
    assert_eq!(info.status, McpStatus::Disconnected);
    assert!(info.tools.is_empty());
    assert!(manager.install_package(bytes).await.is_err());
    let package = root.path().join("_connector_packages/package_fixture");
    assert_eq!(
        std::fs::read(package.join("data.txt")).unwrap(),
        b"resource"
    );
    let saved = std::fs::read_to_string(&store).unwrap();
    assert!(saved.contains(&package.to_string_lossy().to_string()));
    let reloaded = McpManager::new(McpSettings::default())
        .unwrap()
        .with_user_store(store)
        .unwrap();
    assert_eq!(
        reloaded.configured_servers().unwrap()[0].id,
        "package_fixture"
    );
    let traversal = connector_zip(&[("../escaped", b"no")]);
    assert!(manager.install_package(traversal).await.is_err());
    assert!(!root.path().join("escaped").exists());
    let invalid = connector_zip(&[(
        "connector.json",
        br#"{"id":"invalid","command":"../server"}"#,
    )]);
    assert!(manager.install_package(invalid).await.is_err());
    assert!(!root.path().join("_connector_packages/invalid").exists());
    let oversized = connector_zip(&[("big", &vec![0; 16 * 1024 * 1024 + 1])]);
    assert!(manager.install_package(oversized).await.is_err());
    assert_eq!(
        std::fs::read_dir(root.path().join("_connector_packages"))
            .unwrap()
            .count(),
        1
    );
}

#[tokio::test]
async fn installed_package_connects_without_a_project_and_runs_in_project_sandbox() {
    let root = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let secret = tempfile::NamedTempFile::new().unwrap();
    let manager = McpManager::new(McpSettings::default())
        .unwrap()
        .with_user_store(root.path().join("connectors.json"))
        .unwrap();
    let manifest =
        serde_json::to_vec(&json!({"id":"packaged","command":"server","args":[secret.path()]}))
            .unwrap();
    let script = format!(
        "#!/usr/bin/python3\nfrom helper import TOKEN\n{}",
        include_str!("fixtures/mcp_server.py")
    );
    manager
        .install_package(connector_zip(&[
            ("connector.json", &manifest),
            ("server", script.as_bytes()),
            ("helper.py", b"TOKEN = 42\n"),
        ]))
        .await
        .unwrap();
    manager
        .system_action("packaged", McpAction::Connect)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let info = manager.system_list().await.unwrap();
            assert_ne!(info[0].status, McpStatus::Failed, "{:?}", info[0]);
            if info[0].status == McpStatus::Connected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let workspace = project.path().canonicalize().unwrap();
    let mut policy = SandboxPolicy::read_only("project", &workspace);
    if cfg!(target_os = "macos") {
        policy
            .runtime_readonly_paths
            .push("/Library/Developer/CommandLineTools".into());
    }
    let environment = Arc::new(
        ExecutionEnvironment::mxc(
            &workspace,
            &workspace,
            policy,
            env!("CARGO_BIN_EXE_kyler-tool-worker"),
        )
        .unwrap(),
    );
    let tools = manager
        .snapshot("project", Some(environment.clone()))
        .await
        .unwrap();
    assert_eq!(
        tools.len(),
        1,
        "{:?}",
        manager.list("project", Some(environment)).await.unwrap()
    );
    let output: Value = serde_json::from_str(
        &call(tools["mcp__packaged__inspect"].as_ref())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        output["structuredContent"]["cwd"],
        workspace.to_string_lossy().as_ref()
    );
    assert_eq!(output["structuredContent"]["outside_readable"], false);
    assert!(manager.system_list().await.unwrap()[0].removable);
    manager.remove_user_connector("packaged").await.unwrap();
    assert!(manager.system_list().await.unwrap().is_empty());
    assert!(manager.configured_servers().unwrap().is_empty());
    assert!(
        call(tools["mcp__packaged__inspect"].as_ref())
            .await
            .is_err()
    );
    assert!(!root.path().join("_connector_packages/packaged").exists());
    assert!(
        manager
            .system_action("packaged", McpAction::Connect)
            .await
            .is_err()
    );
    manager.shutdown().await;
}
