//! Exercise the actual serve binary, fake model, and native sandbox worker together.
use std::{
    convert::Infallible,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    response::sse::{Event, Sse},
    routing::post,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::TcpListener,
    process::Command,
};

use kyler_client::{Client, ClientEvent};
use kyler_config::ClientSettings;
use kyler_protocol::api::{
    permissions::{PermissionMode, PermissionSelection},
    run::{CreateRunRequest, RunStatus},
};

#[derive(Clone)]
struct Model {
    requests: Arc<Mutex<Vec<Value>>>,
}
fn chunk(delta: Value, reason: Value) -> String {
    json!({"id":"test","object":"chat.completion.chunk","created":0,"model":"test","choices":[{"index":0,"delta":delta,"finish_reason":reason}]}).to_string()
}
async fn completion(
    State(state): State<Model>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Sse<impl futures::Stream<Item = std::result::Result<Event, Infallible>>> {
    assert_eq!(headers["authorization"], "Bearer launch-test-key");
    assert_eq!(request["model"], "launch-model");
    state.requests.lock().unwrap().push(request.clone());
    let messages = request["messages"].as_array().unwrap();
    let current = messages.iter().rposition(|m| m["role"] == "user").unwrap();
    let result = messages[current + 1..]
        .iter()
        .rev()
        .find(|m| m["role"] == "tool");
    let (delta, reason) = if let Some(result) = result {
        (
            json!({"content":result["content"].as_str().unwrap()}),
            json!("stop"),
        )
    } else {
        let input = messages.iter().rev().find(|m| m["role"] == "user").unwrap()["content"]
            .as_str()
            .unwrap();
        let (name, args) = if input == "cargo version" {
            (
                "run_command",
                json!({"program":"cargo","args":["--version"]}),
            )
        } else if input == "rustc version" {
            (
                "run_command",
                json!({"program":"rustc","args":["--version"]}),
            )
        } else if input == "cargo check" {
            (
                "run_command",
                json!({"program":"cargo","args":["check","--offline","--quiet"]}),
            )
        } else if input == "missing command" {
            (
                "run_command",
                json!({"program":"kyler-unavailable-test-tool","args":[]}),
            )
        } else if input == "write" {
            (
                "apply_patch",
                json!({"input":"*** Begin Patch\n*** Add File: created.txt\n+hello\n\n+world\n*** End Patch"}),
            )
        } else if input == "outside" {
            ("list_dir", json!({"dir_path":"/"}))
        } else {
            ("list_dir", json!({"dir_path":"."}))
        };
        (
            json!({"tool_calls":[{"index":0,"id":"files-call","type":"function","function":{"name":name,"arguments":args.to_string()}}]}),
            json!("tool_calls"),
        )
    };
    Sse::new(futures::stream::iter(vec![
        Ok(Event::default().data(chunk(delta, reason))),
        Ok(Event::default().data("[DONE]")),
    ]))
}
async fn run(client: &Client, input: &str, workspace: &Path) -> String {
    let session = client.create_session_in(workspace).await.unwrap();
    let permissions = client.permissions(&session.session_id).await.unwrap();
    let writable = permissions
        .options
        .iter()
        .any(|option| option.mode == PermissionMode::WorkspaceAccess && option.available);
    if !writable {
        for mode in [PermissionMode::WorkspaceAccess, PermissionMode::FullAccess] {
            let error = client
                .select_permissions(
                    &session.session_id,
                    &PermissionSelection {
                        approval: None,
                        mode: Some(mode),
                        confirm_full_access: true,
                    },
                )
                .await
                .unwrap_err();
            assert_eq!(error.status, Some(400));
        }
    }
    client
        .select_permissions(
            &session.session_id,
            &PermissionSelection {
                approval: None,
                mode: Some(if writable {
                    PermissionMode::WorkspaceAccess
                } else {
                    PermissionMode::ReadOnly
                }),
                confirm_full_access: true,
            },
        )
        .await
        .unwrap();
    let run = client
        .start_run(
            &session.session_id,
            &CreateRunRequest {
                attachment_ids: Vec::new(),
                resume: false,
                images: Vec::new(),
                input: input.into(),
                client_request_id: None,
            },
        )
        .await
        .unwrap();
    let mut stream = client.watch(&run.run_id, 0).await.unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(event) = stream.next().await.unwrap() {
            if let ClientEvent::Result(result) = event {
                assert_eq!(result.status, RunStatus::Completed, "{result:?}");
                return result.output.unwrap();
            }
        }
        panic!("no final result")
    })
    .await
    .unwrap()
}
async fn check(workspace_write: bool) {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    let cwd = workspace.join("nested");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(cwd.join("directory-proof.txt"), "proof").unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let model_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let model_url = format!("http://{}/v1", model_listener.local_addr().unwrap());
    let app = Router::new()
        .route("/v1/chat/completions", post(completion))
        .with_state(Model {
            requests: requests.clone(),
        });
    let model = tokio::spawn(async {
        axum::serve(model_listener, app).await.unwrap();
    });
    let config = dir.path().join("kyler.toml");
    std::fs::write(&config,format!("[llm]\nbase_url={model_url:?}\nmodel='test'\napi_key='test'\n[session]\nenabled=true\ndirectory='sessions'\n[server]\nbind='127.0.0.1:0'\n")).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_kyler"));
    command
        .arg("--config")
        .arg(&config)
        .arg("serve")
        .args([
            "--model",
            "launch-model",
            "--base-url",
            &model_url,
            "--api-key",
            "launch-test-key",
        ])
        .arg("--workspace")
        .arg(&workspace)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("KYLER_LLM_BASE_URL", "http://127.0.0.1:1/v1")
        .env("KYLER_LLM_MODEL", "environment-model")
        .env("KYLER_LLM_API_KEY", "environment-key")
        .env_remove("KYLER_SESSION_DIR")
        .env_remove("KYLER_SERVER_BIND")
        .env_remove("KYLER_SESSION_ENABLED")
        .env_remove("KYLER_SERVER_TOKEN")
        .env_remove("KYLER_CLIENT_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if workspace_write {
        command.args(["--sandbox", "workspace-write"]);
    } else {
        command.args(["--sandbox", "read-only"]);
    }
    let mut child = command.spawn().unwrap();
    let mut logs = BufReader::new(child.stderr.take().unwrap()).lines();
    let address = tokio::time::timeout(Duration::from_secs(10), async {
        let mut diagnostic = String::new();
        while let Some(line) = logs.next_line().await.unwrap() {
            if let Some(address) = line.strip_prefix("Kyler server listening on ") {
                return address.to_owned();
            }
            diagnostic.push_str(&line);
        }
        panic!("server failed: {diagnostic}")
    })
    .await
    .unwrap();
    let client = Client::new(ClientSettings {
        server_url: format!("http://{address}"),
        ..Default::default()
    })
    .unwrap();
    let info = client.info().await.unwrap();
    assert_eq!(info.model, "launch-model");
    assert_eq!(info.sliding_window_tokens, Some(76800));
    let listing = run(
        &client,
        "\u{67e5}\u{770b}\u{5f53}\u{524d}\u{76ee}\u{5f55}\u{7ed3}\u{6784}",
        &cwd,
    )
    .await;
    assert!(listing.contains("directory-proof.txt"), "{listing}");
    let first = requests.lock().unwrap()[0].clone();
    let names: Vec<_> = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    for tool in ["list_dir", "read_file", "grep_files"] {
        assert!(names.contains(&tool));
    }
    assert_eq!(names.contains(&"apply_patch"), workspace_write);
    assert!(names.contains(&"run_command"));
    let prompts = first["messages"].to_string();
    let cwd: PathBuf = std::fs::canonicalize(cwd).unwrap();
    assert!(prompts.contains(cwd.to_str().unwrap()));
    assert!(prompts.contains("Reply in the language of the user's current request"));
    // The shared server must not couple concurrent sessions to one directory.
    let other = workspace.join("other-project");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("other-only.txt"), "other").unwrap();
    let (a, b) = tokio::join!(
        run(
            &client,
            "\u{67e5}\u{770b}\u{5f53}\u{524d}\u{76ee}\u{5f55}\u{7ed3}\u{6784}",
            &cwd
        ),
        run(
            &client,
            "\u{67e5}\u{770b}\u{5f53}\u{524d}\u{76ee}\u{5f55}\u{7ed3}\u{6784}",
            &other
        )
    );
    assert!(
        a.contains("directory-proof.txt") && !a.contains("other-only.txt"),
        "{a}"
    );
    assert!(
        b.contains("other-only.txt") && !b.contains("directory-proof.txt"),
        "{b}"
    );
    assert_eq!(
        client
            .create_session_in(dir.path())
            .await
            .unwrap_err()
            .status,
        Some(400)
    );
    let default_session = client.create_session().await.unwrap();
    assert_eq!(
        default_session.workspace_root.as_deref(),
        Some(
            dir.path()
                .join("workspace")
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .as_ref()
        )
    );
    #[cfg(unix)]
    {
        let alias = workspace.join("outside-link");
        std::os::unix::fs::symlink(dir.path(), &alias).unwrap();
        assert_eq!(
            client.create_session_in(&alias).await.unwrap_err().status,
            Some(400)
        );
    }
    let outside = run(&client, "outside", &cwd).await;
    assert!(
        outside.contains("denied") || outside.contains("outside"),
        "{outside}"
    );
    assert!(!outside.contains("Applications/"));
    if workspace_write {
        let result = run(&client, "write", &cwd).await;
        assert!(cwd.join("created.txt").is_file(), "{result}");
        assert!(!workspace.join("created.txt").exists());
        assert_eq!(
            std::fs::read_to_string(cwd.join("created.txt")).unwrap(),
            "hello\n\nworld\n"
        );
        for (input, expected) in [("cargo version", "cargo "), ("rustc version", "rustc ")] {
            let result = run(&client, input, &cwd).await;
            let output: Value = serde_json::from_str(&result).unwrap();
            assert_eq!(output["exit_code"], 0, "{result}");
            assert!(
                output["stdout"].as_str().unwrap().contains(expected),
                "{result}"
            );
        }
        std::fs::write(cwd.join("Cargo.toml"), "[package]\nname='sandbox-runtime-test'\nversion='0.1.0'\nedition='2024'\n[lib]\npath='lib.rs'\n[workspace]\n").unwrap();
        std::fs::write(cwd.join("lib.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
        let result = run(&client, "cargo check", &cwd).await;
        let output: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(output["exit_code"], 0, "{result}");
        assert!(cwd.join("target").is_dir());
        let result = run(&client, "missing command", &cwd).await;
        assert!(result.contains("sandbox PATH"), "{result}");
    }
    let pid = child.id().unwrap();
    assert!(
        Command::new("/bin/kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .await
            .unwrap()
            .success()
    );
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    model.abort();
}
#[cfg(unix)]
#[tokio::test]
async fn serve_binds_readonly_workspace_and_lists_current_directory_in_native_worker() {
    check(false).await;
}
#[cfg(unix)]
#[tokio::test]
async fn serve_workspace_write_enables_patch_with_workspace_relative_paths() {
    check(true).await;
}
