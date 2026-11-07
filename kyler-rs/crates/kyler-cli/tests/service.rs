#![cfg(unix)]
use std::{path::PathBuf, process::Output, time::Duration};

use tokio::{net::TcpListener, process::Command};

use kyler_client::Client;
use kyler_config::{ClientSettings, SecretString};

struct Harness {
    root: tempfile::TempDir,
    config: PathBuf,
    url: String,
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::process::Command::new(env!("CARGO_BIN_EXE_kyler"))
            .arg("--config")
            .arg(&self.config)
            .args(["service", "stop"])
            .env_remove("KYLER_SERVER_TOKEN")
            .output();
    }
}
impl Harness {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join(".kyler");
        std::fs::create_dir(&directory).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let config = directory.join("config.toml");
        std::fs::write(&config,format!("[llm]\nmodel='test'\nbase_url='http://127.0.0.1:1/v1'\napi_key='test-key'\n[session]\nenabled=true\n[server]\nbind='{address}'\naccess_token='service-token'\n")).unwrap();
        Self {
            root,
            config,
            url: format!("http://{address}"),
        }
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kyler"));
        cmd.arg("--config")
            .arg(&self.config)
            .current_dir(self.root.path())
            .kill_on_drop(true);
        for key in [
            "KYLER_CONFIG",
            "KYLER_SERVER_BIND",
            "KYLER_SERVER_TOKEN",
            "KYLER_SESSION_ENABLED",
            "KYLER_SESSION_DIR",
            "KYLER_INTERNAL_TOOL_WORKER",
            "KYLER_LLM_MODEL",
            "KYLER_LLM_BASE_URL",
            "KYLER_LLM_API_KEY",
            "OPENAI_API_KEY",
        ] {
            cmd.env_remove(key);
        }
        cmd
    }
    async fn call(&self, args: &[&str]) -> Output {
        tokio::time::timeout(Duration::from_secs(20), self.command().args(args).output())
            .await
            .unwrap()
            .unwrap()
    }
    fn client(&self) -> Client {
        Client::new(ClientSettings {
            server_url: self.url.clone(),
            access_token: SecretString::new("service-token"),
            ..Default::default()
        })
        .unwrap()
    }
}
#[tokio::test]
async fn managed_background_server_starts_reuses_restarts_and_stops_by_identity() {
    let h = Harness::new().await;
    let out = h.call(&["service", "start"]).await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let client = h.client();
    let first = client.info().await.unwrap();
    assert!(first.workspace_root.is_none());
    let session = client.create_session_in(h.root.path()).await.unwrap();
    assert_eq!(
        session.workspace_root.as_deref(),
        Some(h.root.path().canonicalize().unwrap().to_str().unwrap())
    );
    let out = h.call(&["service", "start"]).await;
    assert!(out.status.success());
    assert_eq!(client.info().await.unwrap().instance_id, first.instance_id);
    let out = h.call(&["--json", "service", "status"]).await;
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["running"],
        true
    );
    let out = h
        .command()
        .args(["service", "start", "--restart"])
        .env("KYLER_LLM_MODEL", "stale-shell-model")
        .env("KYLER_LLM_BASE_URL", "invalid-old-url")
        .env("KYLER_LLM_API_KEY", "old-key")
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let next = client.info().await.unwrap();
    assert_ne!(next.instance_id, first.instance_id);
    assert_eq!(next.model, "test");
    assert_eq!(
        client
            .session(&session.session_id)
            .await
            .unwrap()
            .workspace_root,
        session.workspace_root
    );
    // A stale identity must never shut down the current instance.
    assert_eq!(
        client
            .shutdown_instance(first.instance_id.as_deref().unwrap())
            .await
            .unwrap_err()
            .status,
        Some(409)
    );
    assert!(client.info().await.is_ok());
    let out = h.call(&["service", "stop"]).await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(client.info().await.is_err());
    assert!(!h.config.parent().unwrap().join("server.json").exists());
    assert!(h.config.parent().unwrap().join("logs/server.log").exists());
}
#[tokio::test]
async fn configure_persists_escaped_credentials_enables_sessions_and_preserves_other_settings() {
    let h = Harness::new().await;
    let text = std::fs::read_to_string(&h.config)
        .unwrap()
        .replace("enabled=true", "enabled=false");
    std::fs::write(
        &h.config,
        format!("{text}\n[context]\ncontext_tokens=64000\nwindow_ratio=0.5\n[tools.tavily]\nenabled=true\n"),
    )
    .unwrap();
    let key = "test-key-\"quote\\slash";
    let out = h
        .command()
        .args(["configure", "--non-interactive"])
        .env("KYLER_LLM_API_KEY", key)
        .env("KYLER_LLM_MODEL", "configured-model")
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!String::from_utf8_lossy(&out.stderr).contains(key));
    let config = kyler_config::loader::load_from_with_env(&h.config, &Default::default()).unwrap();
    assert_eq!(config.llm.api_key.expose(), key);
    assert_eq!(config.llm.model, "configured-model");
    assert!(config.session.enabled);
    assert!(
        !std::fs::read_to_string(&h.config)
            .unwrap()
            .contains("tavily")
    );
    assert_eq!(config.context.context_tokens, 64000);
    assert_eq!(config.server.access_token.expose(), "service-token");
    use std::os::unix::fs::PermissionsExt;

    assert_eq!(
        std::fs::metadata(&h.config).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let before = std::fs::read(&h.config).unwrap();
    let out = h
        .command()
        .args(["configure", "--non-interactive"])
        .env("KYLER_LLM_BASE_URL", "invalid")
        .output()
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(std::fs::read(&h.config).unwrap(), before);
    let started = h.call(&["service", "start"]).await;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let info = h.client().info().await.unwrap();
    assert_eq!(info.context_window_tokens, 64000);
    assert_eq!(info.sliding_window_tokens, Some(32000));
}

#[tokio::test]
async fn configure_preserves_named_profiles_and_captures_provider_credentials() {
    let h = Harness::new().await;
    let text = std::fs::read_to_string(&h.config).unwrap();
    std::fs::write(
        &h.config,
        format!(
            r#"{text}
[models]
default = "fast"
[models.providers.remote]
base_url = "https://example.com/v1"
api_key_env = "KYLER_TEST_PROVIDER_KEY"
[models.profiles.fast]
provider = "remote"
model = "test-model"
context_tokens = 64000
output_tokens = 1000
"#
        ),
    )
    .unwrap();
    let out = h
        .command()
        .args(["configure", "--non-interactive"])
        .env("KYLER_TEST_PROVIDER_KEY", "provider-secret")
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!String::from_utf8_lossy(&out.stderr).contains("provider-secret"));
    let config = kyler_config::loader::load_from_with_env(&h.config, &Default::default()).unwrap();
    assert_eq!(config.models.default, "fast");
    assert_eq!(
        config.models.providers["remote"].api_key.expose(),
        "provider-secret"
    );
    assert_eq!(config.models.profiles["fast"].context_tokens, 64000);
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn managed_workspace_access_writes_after_approval_and_read_only_denies_writes() {
    use axum::{
        Json, Router,
        response::sse::{Event, Sse},
        routing::post,
    };
    use serde_json::{Value, json};

    use kyler_protocol::api::{
        permissions::{
            ApprovalDecision, ApprovalResponse, ApprovalStatus, PermissionMode, PermissionSelection,
        },
        run::CreateRunRequest,
    };

    async fn completion(
        Json(request): Json<Value>,
    ) -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
        let messages = request["messages"].as_array().unwrap();
        let user = messages
            .iter()
            .rposition(|message| message["role"] == "user")
            .unwrap();
        let (delta, reason) = if messages[user + 1..]
            .iter()
            .any(|message| message["role"] == "tool")
        {
            (json!({"content":"finished"}), "stop")
        } else {
            let input = messages[user]["content"].as_str().unwrap();
            let (name, arguments) = match input {
                "mkdir" => (
                    "run_command",
                    json!({"program":"/bin/mkdir","args":["-p","assets/nested"]}),
                ),
                "auto edit" => (
                    "apply_patch",
                    json!({"input":"*** Begin Patch\n*** Update File: auto-created.txt\n@@\n-approved\n+updated\n*** End Patch"}),
                ),
                _ => (
                    "apply_patch",
                    json!({"input": format!("*** Begin Patch\n*** Add File: {}\n+approved\n*** End Patch", if input == "auto write" { "auto-created.txt" } else { "managed-created.txt" })}),
                ),
            };
            (
                json!({"tool_calls":[{"index":0,"id":"write-call","type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}),
                "tool_calls",
            )
        };
        Sse::new(futures::stream::iter(vec![Ok(Event::default().data(json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"test","choices":[{"index":0,"delta":delta,"finish_reason":reason}]}).to_string())),Ok(Event::default().data("[DONE]"))]))
    }
    async fn finished(client: &Client, id: &str) {
        tokio::time::timeout(Duration::from_secs(15), async {
            let mut events = client.watch(id, 0).await.unwrap();
            while let Some(event) = events.next().await.unwrap() {
                if matches!(event, kyler_client::ClientEvent::Result(_)) {
                    return;
                }
            }
            panic!("run did not finish");
        })
        .await
        .unwrap();
    }
    let h = Harness::new().await;
    let model_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", model_listener.local_addr().unwrap());
    let model = tokio::spawn(async move {
        axum::serve(
            model_listener,
            Router::new().route("/v1/chat/completions", post(completion)),
        )
        .await
        .unwrap()
    });
    let text = std::fs::read_to_string(&h.config)
        .unwrap()
        .replace("http://127.0.0.1:1/v1", &url);
    std::fs::write(&h.config, text).unwrap();
    let started = h.call(&["service", "start"]).await;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let client = h.client();
    let workspace = h.root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let session = client.create_session_in(&workspace).await.unwrap();
    let state = client.permissions(&session.session_id).await.unwrap();
    assert!(
        state
            .options
            .iter()
            .any(|option| option.mode == PermissionMode::WorkspaceAccess && option.available)
    );
    for mode in [PermissionMode::ReadOnly, PermissionMode::WorkspaceAccess] {
        client
            .select_permissions(
                &session.session_id,
                &PermissionSelection {
                    approval: if mode == PermissionMode::WorkspaceAccess {
                        Some(kyler_protocol::approval::ApprovalSettings {
                            rules: std::collections::BTreeMap::from([(
                                "apply_patch".into(),
                                kyler_protocol::approval::ToolRule::Ask,
                            )]),
                            ..Default::default()
                        })
                    } else {
                        None
                    },
                    mode: Some(mode),
                    confirm_full_access: false,
                },
            )
            .await
            .unwrap();
        let run = client
            .start_run(
                &session.session_id,
                &CreateRunRequest {
                    input: "write file".into(),
                    client_request_id: None,
                    images: Vec::new(),
                    attachment_ids: Vec::new(),
                    resume: false,
                },
            )
            .await
            .unwrap();
        if mode == PermissionMode::WorkspaceAccess {
            let approval = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Some(record) = client
                        .approvals(&session.session_id)
                        .await
                        .unwrap()
                        .into_iter()
                        .find(|record| record.status == ApprovalStatus::Pending)
                    {
                        break record;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            client
                .decide_approval(
                    &approval.id,
                    &ApprovalResponse {
                        decision: ApprovalDecision::AllowOnce,
                        digest: approval.digest,
                    },
                )
                .await
                .unwrap();
        }
        finished(&client, &run.run_id).await;
        assert_eq!(
            workspace.join("managed-created.txt").exists(),
            mode == PermissionMode::WorkspaceAccess
        );
    }
    assert_eq!(
        std::fs::read_to_string(workspace.join("managed-created.txt")).unwrap(),
        "approved\n"
    );
    client
        .select_permissions(
            &session.session_id,
            &PermissionSelection {
                approval: None,
                mode: Some(PermissionMode::AutoReview),
                confirm_full_access: false,
            },
        )
        .await
        .unwrap();
    for input in ["mkdir", "auto write", "auto edit"] {
        let run = client
            .start_run(
                &session.session_id,
                &CreateRunRequest {
                    input: input.into(),
                    client_request_id: None,
                    images: Vec::new(),
                    attachment_ids: Vec::new(),
                    resume: false,
                },
            )
            .await
            .unwrap();
        finished(&client, &run.run_id).await;
        let records = client.approvals(&session.session_id).await.unwrap();
        let records = records
            .iter()
            .filter(|record| record.run_id == run.run_id)
            .collect::<Vec<_>>();
        assert!(!records.is_empty());
        assert!(
            records
                .iter()
                .all(|record| record.status == ApprovalStatus::Allowed && !record.requires_user)
        );
    }
    assert!(workspace.join("assets/nested").is_dir());
    assert_eq!(
        std::fs::read_to_string(workspace.join("auto-created.txt")).unwrap(),
        "updated\n"
    );
    model.abort();
}
