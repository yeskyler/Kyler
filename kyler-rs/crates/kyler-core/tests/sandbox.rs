//! Native enforcement tests: no ignored tests or mock fallbacks on supported macOS.
#![cfg(target_os = "macos")]
use std::{
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

use kyler_core::{
    Agent, ExecutionEnvironment, NetworkAccess, SandboxPolicy, Session, ToolBox,
    attachments::AttachmentStore, processes::ProcessManager, sandbox::ProcessInput,
};
use kyler_protocol::api::attachments::ReadAttachmentRequest;

fn worker() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_kyler-tool-worker"))
}
fn environment(root: &Path, cwd: &Path, policy: SandboxPolicy) -> Arc<ExecutionEnvironment> {
    Arc::new(
        ExecutionEnvironment::mxc(root, cwd, policy, worker())
            .expect("native MXC backend must be available"),
    )
}
async fn command(
    env: &ExecutionEnvironment,
    program: &str,
    args: Vec<String>,
) -> anyhow::Result<Value> {
    Ok(serde_json::from_str(
        &env.run_tool(
            "run_command",
            &json!({"program":program,"args":args}).to_string(),
        )
        .await?,
    )?)
}
#[tokio::test]
async fn native_file_permissions_symlinks_and_patch_preflight() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, "OUTSIDE_SECRET").unwrap();
    std::fs::create_dir(root.join("readonly")).unwrap();
    std::fs::write(root.join("readonly/keep.txt"), "original").unwrap();
    std::fs::write(root.join(".env"), "PRIVATE_ENV").unwrap();
    std::os::unix::fs::symlink(&secret, root.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("escape")).unwrap();
    let mut policy = SandboxPolicy::workspace_write("workspace", &root);
    policy.filesystem.readonly_paths.push(root.join("readonly"));
    policy.filesystem.denied_paths.push(root.join(".env"));
    if let Some(paths) = std::env::var_os("PATH") {
        if let Some(rg) = std::env::split_paths(&paths)
            .map(|p| p.join("rg"))
            .find(|p| p.is_file())
        {
            let rg = rg.canonicalize().unwrap();
            policy.executable_search_path = format!(
                "{}:{}",
                rg.parent().unwrap().display(),
                policy.executable_search_path
            );
            policy.runtime_readonly_paths.push(rg);
        }
    }
    let env = environment(&root, &root, policy);
    env.run_tool(
        "apply_patch",
        &json!({"input":"*** Begin Patch\n*** Add File: ok.txt\n+hello\n*** End Patch"})
            .to_string(),
    )
    .await
    .unwrap();
    let read = env
        .run_tool("read_file", &json!({"file_path":"ok.txt"}).to_string())
        .await
        .unwrap();
    assert!(read.contains("L1: hello"));
    let listing = env
        .run_tool("list_dir", &json!({"dir_path":".","depth":1}).to_string())
        .await
        .unwrap();
    assert!(listing.contains("ok.txt"));
    let matches = env
        .run_tool(
            "grep_files",
            &json!({"pattern":"hello","path":".","include":"*.txt"}).to_string(),
        )
        .await
        .unwrap();
    assert!(matches.contains("ok.txt"));
    for path in [&secret, &root.join("link.txt"), &root.join(".env")] {
        assert!(
            env.run_tool("read_file", &json!({"file_path":path}).to_string())
                .await
                .is_err()
        );
        // Commands bypass the application path validator, proving kernel enforcement.
        let result = command(&env, "/bin/cat", vec![path.to_string_lossy().into_owned()])
            .await
            .unwrap();
        assert_ne!(result["exit_code"], 0);
        assert!(!result["stdout"].as_str().unwrap().contains("SECRET"));
    }
    let denied = command(
        &env,
        "/bin/sh",
        vec!["-c".into(), "printf hacked > readonly/keep.txt".into()],
    )
    .await
    .unwrap();
    assert_ne!(denied["exit_code"], 0);
    assert_eq!(
        std::fs::read_to_string(root.join("readonly/keep.txt")).unwrap(),
        "original"
    );
    let patch = "*** Begin Patch\n*** Add File: preflight.txt\n+must not be written\n*** Add File: escape/new.txt\n+bad\n*** End Patch";
    assert!(
        env.run_tool("apply_patch", &json!({"input":patch}).to_string())
            .await
            .is_err()
    );
    assert!(!root.join("preflight.txt").exists());
    assert!(!outside.path().join("new.txt").exists());
}
#[tokio::test]
async fn working_directories_are_per_request_and_patch_is_workspace_relative() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("a")).unwrap();
    std::fs::create_dir(root.join("b")).unwrap();
    let a = environment(
        &root,
        &root.join("a"),
        SandboxPolicy::workspace_write("a", &root),
    );
    let b = environment(
        &root,
        &root.join("b"),
        SandboxPolicy::workspace_write("b", &root),
    );
    let original = std::env::current_dir().unwrap();
    let (first, second) = tokio::join!(
        command(&a, "/bin/pwd", vec![]),
        command(&b, "/bin/pwd", vec![])
    );
    assert_eq!(
        first.unwrap()["stdout"].as_str().unwrap().trim(),
        root.join("a").to_str().unwrap()
    );
    assert_eq!(
        second.unwrap()["stdout"].as_str().unwrap().trim(),
        root.join("b").to_str().unwrap()
    );
    assert_eq!(std::env::current_dir().unwrap(), original);
    a.run_tool(
        "apply_patch",
        &json!({"input":"*** Begin Patch\n*** Add File: root.txt\n+root\n*** End Patch"})
            .to_string(),
    )
    .await
    .unwrap();
    assert!(root.join("root.txt").exists());
    assert!(!root.join("a/root.txt").exists());
    let absolute = root.join("absolute.txt");
    a.run_tool("apply_patch", &json!({"input":format!("*** Begin Patch\n*** Add File: {}\n+absolute\n*** End Patch", absolute.display())}).to_string()).await.unwrap();
    assert_eq!(std::fs::read_to_string(&absolute).unwrap(), "absolute\n");
    let outside = tempfile::tempdir().unwrap();
    let denied = outside.path().join("must-not-exist.txt");
    assert!(a.run_tool("apply_patch", &json!({"input":format!("*** Begin Patch\n*** Add File: {}\n+bad\n*** End Patch", denied.display())}).to_string()).await.is_err());
    assert!(!denied.exists());
    let vars = command(&a, "/usr/bin/env", vec![]).await.unwrap();
    let vars = vars["stdout"].as_str().unwrap();
    assert!(!vars.lines().any(|s| s.starts_with("SSH_AUTH_SOCK=")
        || s.starts_with("USER=")
        || s.starts_with("OPENAI_API_KEY=")));
}
#[tokio::test]
async fn native_network_deny_blocks_a_working_local_http_server() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let received = count.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            received.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 22\r\nConnection: close\r\n\r\nNETWORK_ALLOWED_SECRET").await.unwrap();
            });
        }
    });
    assert_eq!(
        reqwest::get(&url).await.unwrap().text().await.unwrap(),
        "NETWORK_ALLOWED_SECRET"
    );
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let env = environment(
        &root,
        &root,
        SandboxPolicy::workspace_write("deny-network", &root),
    );
    let result = command(
        &env,
        "/usr/bin/curl",
        vec![
            "--connect-timeout".into(),
            "1".into(),
            "--max-time".into(),
            "2".into(),
            url,
        ],
    )
    .await
    .unwrap();
    assert_ne!(result["exit_code"], 0);
    assert!(
        !result["stdout"]
            .as_str()
            .unwrap()
            .contains("NETWORK_ALLOWED_SECRET")
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
}
#[tokio::test]
async fn timeout_output_limit_and_future_cancellation_clean_up_processes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut policy = SandboxPolicy::workspace_write("timeout", &root);
    policy.process.timeout_ms = 300;
    let env = environment(&root, &root, policy);
    let timed = command(
        &env,
        "/bin/sh",
        vec!["-c".into(), "sleep 5; echo bad > leaked-timeout".into()],
    )
    .await;
    assert!(format!("{:#}", timed.unwrap_err()).contains("tim"));
    assert!(!root.join("leaked-timeout").exists());
    let mut policy = SandboxPolicy::workspace_write("output", &root);
    policy.process.max_output_bytes = 512;
    let env = environment(&root, &root, policy);
    let output = command(&env, "/usr/bin/yes", vec!["payload".into()]).await;
    assert!(format!("{:#}", output.unwrap_err()).contains("limit"));
    let env = environment(
        &root,
        &root,
        SandboxPolicy::workspace_write("cancel", &root),
    );
    let running = env.clone();
    let task = tokio::spawn(async move {
        command(
            &running,
            "/bin/sh",
            vec![
                "-c".into(),
                "echo $$ > child.pid; sleep 30; echo bad > leaked-cancel".into(),
            ],
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !root.join("child.pid").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let pid = std::fs::read_to_string(root.join("child.pid")).unwrap();
    let pid = pid.trim().to_string();
    task.abort();
    let _ = task.await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let alive = Command::new("/bin/kill")
                .args(["-0", &pid])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success();
            if !alive {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(!root.join("leaked-cancel").exists());
}
#[tokio::test]
async fn persisted_environment_cannot_authorize_unsandboxed_or_different_execution() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let env = environment(&root, &root, SandboxPolicy::read_only("locked", &root));
    let mut session = Session::new();
    session.environment = Some(env.session_environment().clone());
    let agent = Agent::new(
        String::new(),
        String::new(),
        "test",
        Some("help"),
        Arc::new(ToolBox::new()),
    );
    assert!(
        agent
            .run_in_session(&mut session, "test")
            .await
            .unwrap_err()
            .to_string()
            .contains("host-authorized")
    );
    let different = environment(
        &root,
        &root,
        SandboxPolicy::workspace_write("different", &root),
    );
    assert!(
        agent
            .with_environment(different)
            .run_in_session(&mut session, "test")
            .await
            .unwrap_err()
            .to_string()
            .contains("differs")
    );
    assert!(session.events.is_empty());
}

#[tokio::test]
async fn proxy_policy_allows_proxy_but_not_direct_host_connections() {
    async fn server(body: &'static str) -> (String, Arc<AtomicUsize>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let handle = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        (url, calls, handle)
    }
    let (proxy, proxied, proxy_server) = server("PROXY_OK").await;
    let (direct, direct_calls, direct_server) = server("FORBIDDEN_DIRECT").await;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut policy = SandboxPolicy::workspace_write("proxy-only", &root);
    policy.network = NetworkAccess::Proxy { url: proxy };
    let env = environment(&root, &root, policy);
    let result = command(
        &env,
        "/usr/bin/curl",
        vec![
            "--max-time".into(),
            "2".into(),
            "http://example.invalid/".into(),
        ],
    )
    .await
    .unwrap();
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["stdout"], "PROXY_OK");
    assert_eq!(proxied.load(Ordering::SeqCst), 1);
    let bypass = command(
        &env,
        "/usr/bin/curl",
        vec![
            "--noproxy".into(),
            "*".into(),
            "--connect-timeout".into(),
            "1".into(),
            direct,
        ],
    )
    .await
    .unwrap();
    assert_ne!(bypass["exit_code"], 0);
    assert_eq!(direct_calls.load(Ordering::SeqCst), 0);
    proxy_server.abort();
    direct_server.abort();
}

#[tokio::test]
async fn tool_does_not_fall_back_to_host_when_backend_fails() {
    use kyler_core::{
        Tool, ToolContext,
        sandbox::{Sandbox, SandboxRequest, WorkerResponse},
        tools::read_file::ReadFileTool,
    };

    struct FailingBackend;
    #[async_trait::async_trait]
    impl Sandbox for FailingBackend {
        async fn run(&self, _: SandboxRequest) -> anyhow::Result<WorkerResponse> {
            anyhow::bail!("backend unavailable sentinel")
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    std::fs::write(root.join("file.txt"), "HOST_READ_MUST_NOT_HAPPEN").unwrap();
    let env = ExecutionEnvironment::new(
        &root,
        &root,
        SandboxPolicy::read_only("failed", &root),
        worker(),
        Arc::new(FailingBackend),
    )
    .unwrap();
    let mut session = Session::new();
    session.begin_execution("read").unwrap();
    let mut context = ToolContext {
        processes: None,
        execution: session.execution.as_mut().unwrap(),
        state: &mut session.state,
        session_id: &session.session_id,
        tool_call_id: "test",
        subagents: None,
        environment: Some(&env),
    };
    let error = ReadFileTool
        .execute(
            &json!({"file_path":root.join("file.txt")}).to_string(),
            &mut context,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("sentinel"));
}

#[tokio::test]
async fn managed_process_streams_before_exit_accepts_input_and_cancels_in_sandbox() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let policy = SandboxPolicy::workspace_write("process-test", &root);
    let env = environment(&root, &root, policy);
    let manager = ProcessManager::default();
    let mut events = manager.subscribe();
    let id=manager.start("owner",env.as_ref().clone(),json!({"program":"/bin/sh","args":["-c","printf ready; read value; printf 'received:%s' \"$value\""]})).unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(first.text.contains("ready"));
    assert_eq!(manager.read("owner", &id, 0).unwrap().status, "running");
    assert!(manager.read("other", &id, 0).is_err());
    manager
        .write(
            "owner",
            &id,
            ProcessInput {
                data: "hello\n".into(),
                close: true,
            },
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let info = manager.read("owner", &id, 0).unwrap();
            if info.status != "running" {
                assert_eq!(info.status, "completed", "{:?}", info.error);
                assert!(info.result.unwrap().contains("received:hello"));
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let id = manager
        .start(
            "owner",
            env.as_ref().clone(),
            json!({"program":"/bin/sh","args":["-c","sleep 1; touch escaped"]}),
        )
        .unwrap();
    manager.stop("owner", &id).unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(!root.join("escaped").exists());
    manager.shutdown();
}

#[tokio::test]
async fn attachment_document_reads_are_isolated_and_do_not_expose_store_paths() {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    let store_root = tempfile::tempdir().unwrap();
    let session = uuid::Uuid::new_v4().to_string();
    let store = AttachmentStore::new(store_root.path());
    let info = store
        .put(&session, "notes.txt", b"line one\nline two")
        .unwrap();
    let mut policy = SandboxPolicy::workspace_write("attachment-test", &root_path);
    policy
        .filesystem
        .denied_paths
        .push(store_root.path().canonicalize().unwrap());
    let env = environment(&root_path, &root_path, policy);
    let content = store
        .read_isolated(
            &session,
            &ReadAttachmentRequest {
                attachment_id: info.id.clone(),
                offset: 1,
                limit: Some(1),
                ..Default::default()
            },
            &env,
        )
        .await
        .unwrap();
    assert_eq!(content.content, "line two");
    assert_eq!(content.info.id, info.id);
    assert!(
        !serde_json::to_string(&content)
            .unwrap()
            .contains(store_root.path().to_str().unwrap())
    );
}

#[tokio::test]
async fn unrestricted_and_external_workers_access_outside_workspace_and_local_network() {
    for policy in [
        SandboxPolicy::danger_full_access("full-test"),
        SandboxPolicy::external_sandbox("external-test"),
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("outside.txt");
        let env = environment(&root, &root, policy);
        let result = command(
            &env,
            "/usr/bin/touch",
            vec![target.to_string_lossy().into_owned()],
        )
        .await
        .unwrap();
        assert_eq!(result["exit_code"], 0);
        assert!(target.exists());
        let listing = env
            .run_tool(
                "list_dir",
                &json!({"dir_path":outside.path(),"depth":1,"limit":10}).to_string(),
            )
            .await
            .unwrap();
        assert!(listing.contains("outside.txt"));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 2048];
            let _ = socket.read(&mut buffer).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
                .await
                .unwrap();
        });
        let response = command(
            &env,
            "/usr/bin/curl",
            vec!["--silent".into(), "--max-time".into(), "2".into(), url],
        )
        .await
        .unwrap();
        assert_eq!(response["exit_code"], 0);
        assert_eq!(response["stdout"], "OK");
        server.await.unwrap();
        let restricted = env.read_only();
        assert!(restricted.run_tool("apply_patch", &json!({"input":"*** Begin Patch\n*** Add File: blocked.txt\n+blocked\n*** End Patch"}).to_string()).await.is_err());
        assert!(!root.join("blocked.txt").exists());
    }
}

#[tokio::test]
async fn readonly_worker_cannot_write_workspace_or_its_temporary_directory() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let env = environment(
        &root,
        &root,
        SandboxPolicy::read_only("strict-read-only", &root),
    );
    let denied = command(
        &env,
        "/bin/sh",
        vec![
            "-c".into(),
            "touch workspace-write; touch \"$TMPDIR/temp-write\"".into(),
        ],
    )
    .await
    .unwrap();
    assert_ne!(denied["exit_code"], 0);
    assert!(!root.join("workspace-write").exists());
    assert!(!denied["stderr"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn direct_worker_timeout_and_cancellation_reap_the_process_group() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let mut policy = SandboxPolicy::danger_full_access("direct-timeout");
    policy.process.timeout_ms = 100;
    let env = environment(&root, &root, policy);
    assert!(
        command(&env, "/bin/sleep", vec!["10".into()])
            .await
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    let env = environment(
        &root,
        &root,
        SandboxPolicy::external_sandbox("direct-cancel"),
    );
    let pidfile = root.join("child.pid");
    let executor = env.clone();
    let path = pidfile.clone();
    let task = tokio::spawn(async move {
        command(
            &executor,
            "/bin/sh",
            vec![
                "-c".into(),
                "echo $$ > \"$1\"; sleep 10".into(),
                "test".into(),
                path.to_string_lossy().into_owned(),
            ],
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !pidfile.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let pid: i32 = std::fs::read_to_string(pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    task.abort();
    let _ = task.await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while unsafe { libc::kill(pid, 0) } == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn workspace_write_keeps_extra_roots_temp_and_protected_paths_distinct() {
    let temporary = tempfile::tempdir().unwrap();
    let base = temporary.path().canonicalize().unwrap();
    let root = base.join("workspace");
    let extra = base.join("extra");
    let readonly = base.join("readonly");
    for path in [&root, &extra, &readonly, &root.join(".git")] {
        std::fs::create_dir_all(path).unwrap();
    }
    let mut policy = SandboxPolicy::workspace_write("extra-roots", &root);
    policy
        .filesystem
        .readonly_paths
        .extend([base.clone(), root.join(".git")]);
    policy
        .filesystem
        .write_protected_paths
        .push(root.join(".git"));
    policy.filesystem.readwrite_paths.push(extra.clone());
    let env = environment(&root, &root, policy);
    for target in [root.join("allowed"), extra.join("allowed")] {
        assert!(env.policy().filesystem.check_write(&root, &target).is_ok());
        let result = command(
            &env,
            "/usr/bin/touch",
            vec![target.to_string_lossy().into_owned()],
        )
        .await
        .unwrap();
        assert_eq!(result["exit_code"], 0, "{result}");
    }
    for target in [readonly.join("blocked"), root.join(".git/blocked")] {
        assert!(env.policy().filesystem.check_write(&root, &target).is_err());
        let result = command(
            &env,
            "/usr/bin/touch",
            vec![target.to_string_lossy().into_owned()],
        )
        .await
        .unwrap();
        assert_ne!(result["exit_code"], 0);
        assert!(!target.exists());
    }
    let result = command(
        &env,
        "/bin/sh",
        vec!["-c".into(), "touch \"$TMPDIR/allowed\"".into()],
    )
    .await
    .unwrap();
    assert_eq!(result["exit_code"], 0);
}

#[tokio::test]
async fn full_access_can_narrow_to_workspace_without_losing_protected_paths() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_path = outside.path().join("outside.txt");
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::write(root.join("server-secret"), "private").unwrap();
    let host = environment(
        &root,
        &root,
        SandboxPolicy::danger_full_access("preset-test"),
    );
    let host = host
        .with_protected_paths(&[root.join("server-secret")])
        .unwrap();
    let workspace = host.workspace_access();
    assert_eq!(
        workspace.policy().mode,
        kyler_core::sandbox::SandboxMode::WorkspaceWrite
    );
    assert!(matches!(
        workspace.policy().network,
        kyler_core::sandbox::NetworkAccess::AllowOutbound
    ));
    let inside = root.join("inside.txt");
    assert_eq!(
        command(
            &workspace,
            "/usr/bin/touch",
            vec![inside.to_string_lossy().into_owned()]
        )
        .await
        .unwrap()["exit_code"],
        0
    );
    assert!(inside.exists());
    assert_ne!(
        command(
            &workspace,
            "/usr/bin/touch",
            vec![outside_path.to_string_lossy().into_owned()]
        )
        .await
        .unwrap()["exit_code"],
        0
    );
    assert!(!outside_path.exists());
    assert_ne!(
        command(
            &workspace,
            "/usr/bin/touch",
            vec![root.join(".git/blocked").to_string_lossy().into_owned()]
        )
        .await
        .unwrap()["exit_code"],
        0
    );
    assert!(
        workspace
            .run_tool(
                "read_file",
                &json!({"file_path":root.join("server-secret")}).to_string()
            )
            .await
            .is_err()
    );
    let readonly = host.read_only();
    assert!(
        readonly
            .run_tool(
                "read_file",
                &json!({"file_path":root.join("server-secret")}).to_string()
            )
            .await
            .is_err()
    );
    assert_eq!(
        command(
            &host,
            "/usr/bin/touch",
            vec![outside_path.to_string_lossy().into_owned()]
        )
        .await
        .unwrap()["exit_code"],
        0
    );
    assert!(
        outside_path.exists(),
        "Narrowing must never mutate the host ceiling"
    );
}

#[tokio::test]
async fn rustup_runtime_stays_readonly_and_does_not_expose_cargo_credentials() {
    use std::os::unix::fs::PermissionsExt;

    use kyler_core::sandbox::policy::RustupRuntime;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let runtime_home = runtime.path().canonicalize().unwrap();
    let binary = runtime_home.join("rustup");
    std::fs::write(&binary, "#!/bin/sh\nprintf 'fixture cargo\\n'\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let settings = runtime_home.join("settings.toml");
    std::fs::write(&settings, "host settings").unwrap();
    let credentials = tempfile::tempdir().unwrap();
    let credential_path = credentials.path().join("credentials.toml");
    std::fs::write(&credential_path, "PRIVATE_CARGO_CREDENTIAL").unwrap();
    let mut policy = SandboxPolicy::workspace_write("readonly-rust-runtime", &root);
    policy.runtime_readonly_paths = vec![runtime_home.clone()];
    policy.rustup = Some(RustupRuntime {
        home: runtime_home.clone(),
        binary,
        toolchain: None,
    });
    let env = environment(&root, &root, policy);
    let version = command(&env, "cargo", vec!["--version".into()])
        .await
        .unwrap();
    assert_eq!(version["exit_code"], 0);
    let write = command(
        &env,
        "/bin/sh",
        vec![
            "-c".into(),
            "printf changed > \"$RUSTUP_HOME/settings.toml\"".into(),
        ],
    )
    .await
    .unwrap();
    assert_ne!(write["exit_code"], 0);
    assert_eq!(std::fs::read_to_string(settings).unwrap(), "host settings");
    let read = command(
        &env,
        "/bin/cat",
        vec![credential_path.to_string_lossy().into_owned()],
    )
    .await
    .unwrap();
    assert_ne!(read["exit_code"], 0);
    assert!(
        !read["stdout"]
            .as_str()
            .unwrap()
            .contains("PRIVATE_CARGO_CREDENTIAL")
    );
}

#[tokio::test]
async fn project_command_environment_survives_calls_and_is_shared_with_background_processes() {
    use kyler_core::processes::ProcessManager;
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("project")).unwrap();
    std::fs::create_dir(root.join("project/bin")).unwrap();
    // A local executable stands in for any project-managed runtime; no downloads are needed.
    std::fs::write(
        root.join("project/bin/runtime"),
        "#!/bin/sh\nprintf '%s:%s' \"$PROJECT_RUNTIME\" \"$PWD\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        root.join("project/bin/runtime"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let env = environment(
        &root,
        &root,
        SandboxPolicy::workspace_write("task-environment", &root),
    );
    let arguments = json!({"program":"./bin/runtime", "working_directory":"project", "environment":{"PROJECT_RUNTIME":"local"}, "timeout_ms":3000});
    for _ in 0..2 {
        let result: Value = serde_json::from_str(
            &env.run_tool("run_command", &arguments.to_string())
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["exit_code"], 0);
        assert_eq!(
            result["stdout"],
            format!("local:{}", root.join("project").display())
        );
        assert_eq!(
            result["working_directory"],
            root.join("project").to_string_lossy().as_ref()
        );
    }
    let clean: Value = serde_json::from_str(&env.run_tool("run_command", &json!({"program":"/bin/sh", "args":["-c", "printf '%s:%s' \"${PROJECT_RUNTIME-unset}\" \"${KYLER_INTERNAL_TOOL_WORKER-unset}\""]}).to_string()).await.unwrap()).unwrap();
    assert_eq!(clean["stdout"], "unset:unset");
    assert_eq!(clean["working_directory"], root.to_string_lossy().as_ref());
    let manager = ProcessManager::default();
    let mut session = Session::new();
    session.begin_execution("project runtime").unwrap();
    let mut context = kyler_core::tools::ToolContext {
        processes: Some(&manager),
        execution: session.execution.as_mut().unwrap(),
        state: &mut session.state,
        session_id: &session.session_id,
        tool_call_id: "project-runtime",
        subagents: None,
        environment: Some(&env),
    };
    use kyler_core::tools::Tool;
    let tool = kyler_core::tools::shell::RunCommandTool;
    let mut arguments = arguments;
    arguments["yield_time_ms"] = json!(0);
    let started: Value = serde_json::from_str(
        &tool
            .execute(&arguments.to_string(), &mut context)
            .await
            .unwrap(),
    )
    .unwrap();
    let id = started["process_id"].as_str().unwrap();
    let finished = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let info = manager.read(&session.session_id, id, 0).unwrap();
            if info.status != "running" {
                break info;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(finished.error.is_none(), "{:?}", finished.error);
    assert!(serde_json::to_string(&finished).unwrap().contains("local:"));
    for invalid in [
        json!({"program":"/bin/pwd", "working_directory":"../escape"}),
        json!({"program":"/bin/pwd", "working_directory":"missing"}),
        json!({"program":"/bin/pwd", "environment":{"KYLER_INTERNAL_TOOL_WORKER":"1"}}),
        json!({"program":"/bin/pwd", "timeout_ms":30001}),
    ] {
        assert!(
            env.run_tool("run_command", &invalid.to_string())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn unified_commands_complete_yield_accept_input_and_preserve_valid_large_results() {
    use kyler_core::tools::shell::RunCommandArgs;
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let env = environment(
        &root,
        &root,
        SandboxPolicy::workspace_write("unified-command", &root),
    );
    let manager = ProcessManager::default();
    let args = |value| serde_json::from_value::<RunCommandArgs>(value).unwrap();
    let quick = manager.run_command("owner", (*env).clone(), args(json!({"program":"/bin/sh", "args":["-c", "printf done; exit 7"], "yield_time_ms":5000}))).await.unwrap();
    assert_eq!(quick["status"], "completed");
    assert_eq!(quick["exit_code"], 7);
    assert_eq!(quick["stdout"], "done");
    let running = manager.run_command("owner", (*env).clone(), args(json!({"program":"/bin/sh", "args":["-c", "read value; printf 'input:%s' \"$value\""], "yield_time_ms":30}))).await.unwrap();
    assert_eq!(running["status"], "running");
    let id = running["process_id"].as_str().unwrap();
    assert!(manager.read("other-session", id, 0).is_err());
    manager
        .write(
            "owner",
            id,
            ProcessInput {
                data: "hello\n".into(),
                close: true,
            },
        )
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let result = manager.read("owner", id, 0).unwrap();
            if result.status != "running" {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let result: Value = serde_json::from_str(result.result.as_deref().unwrap()).unwrap();
    assert_eq!(result["stdout"], "input:hello");
    let large = manager.run_command("owner", (*env).clone(), args(json!({"program":"/bin/sh", "args":["-c", "i=0; while [ $i -lt 20000 ]; do printf x; i=$((i+1)); done"], "yield_time_ms":5000}))).await.unwrap();
    assert_eq!(large["status"], "completed");
    assert_eq!(large["stdout_truncated"], true);
    assert_eq!(large["exit_code"], 0);
    let long = manager.run_command("owner", (*env).clone(), args(json!({"program":"/bin/sh", "args":["-c", "sleep 5; touch must-not-exist"], "yield_time_ms":0}))).await.unwrap();
    let id = long["process_id"].as_str().unwrap();
    manager.stop("owner", id).unwrap();
    assert_eq!(manager.read("owner", id, 0).unwrap().status, "cancelled");
    assert!(!root.join("must-not-exist").exists());
    let timeout = manager.run_command("owner", (*env).clone(), args(json!({"program":"/bin/sh", "args":["-c", "sleep 5"], "timeout_ms":100, "yield_time_ms":5000}))).await;
    assert!(timeout.is_err());
    let waiting_manager = manager.clone();
    let waiting_env = (*env).clone();
    let waiting = tokio::spawn(async move {
        waiting_manager.run_command("waiting-owner", waiting_env, serde_json::from_value(json!({"program":"/bin/sh", "args":["-c", "sleep 5; touch cancelled-wait"], "yield_time_ms":5000})).unwrap()).await
    });
    let id = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(process) = manager.list("waiting-owner").first() {
                break process.id.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    waiting.abort();
    let _ = waiting.await;
    assert_eq!(
        manager.read("waiting-owner", &id, 0).unwrap().status,
        "cancelled"
    );
    assert!(!root.join("cancelled-wait").exists());
    manager.shutdown();
}

#[tokio::test]
async fn native_review_reads_git_with_no_command_tool_or_repository_helpers() {
    use kyler_core::subagent::{DelegateTaskArgs, SubagentExecutor};
    use kyler_protocol::review::{AgentKind, ReviewRequest, ReviewTarget};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.name", "Review fixture"]);
    git(&["config", "user.email", "review@example.invalid"]);
    std::fs::write(root.join("code.rs"), "pub fn value() -> u32 { 1 }\n").unwrap();
    git(&["add", "code.rs"]);
    git(&["commit", "-qm", "base"]);
    std::fs::write(root.join("code.rs"), "pub fn value() -> u32 { 2 }\n").unwrap();
    git(&[
        "config",
        "filter.evil.clean",
        &format!("touch {}", root.join("FORBIDDEN_HELPER").display()),
    ]);
    std::fs::write(root.join(".gitattributes"), "*.rs filter=evil\n").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let capture = calls.clone();
    let router = axum::Router::new().route("/v1/chat/completions",axum::routing::post(move |axum::Json(request):axum::Json<Value>| {
        let capture = capture.clone();
        async move {
            let names = request["tools"].as_array().unwrap().iter().map(|tool| tool["function"]["name"].as_str().unwrap()).collect::<Vec<_>>();
            assert_eq!(names.len(),3);assert!(!names.contains(&"run_command") && !names.contains(&"apply_patch"));
            let message = if capture.fetch_add(1,Ordering::SeqCst) == 0 { json!({"role":"assistant","tool_calls":[{"id":"read","type":"function","function":{"name":"read_review_file","arguments":"{\"path\":\"code.rs\"}"}}]}) }
                else { json!({"role":"assistant","content":json!({"assessment":"no_findings","summary":"No actionable defect found.","findings":[],"coverage":{"status":"complete","limitations":[]}}).to_string()}) };
            axum::Json(json!({"id":"fixture","object":"chat.completion","created":0,"model":"test","choices":[{"index":0,"message":message,"finish_reason":"stop"}]}))
        }
    }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let model = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let env = environment(
        &root,
        &root,
        SandboxPolicy::workspace_write("review-native", &root),
    );
    assert!(
        env.read_only()
            .run_tool("run_command", r#"{"program":"/usr/bin/true"}"#)
            .await
            .is_err()
    );
    let runtime = Agent::new(
        url,
        "fixture".into(),
        "test",
        None::<String>,
        Arc::new(ToolBox::new()),
    )
    .with_environment(env);
    let executor = runtime
        .for_task(&DelegateTaskArgs {
            agent: AgentKind::Review,
            review: Some(ReviewRequest {
                target: ReviewTarget::Uncommitted,
                paths: vec!["code.rs".into()],
                include_untracked: true,
            }),
            task: "Review the code change".into(),
            context: None,
        })
        .unwrap()
        .unwrap();
    let mut session = Session::new();
    session.environment = executor.session_environment();
    let result = executor
        .execute_result(&mut session, r#"{"task":"Review the code change"}"#)
        .await
        .unwrap();
    assert!(result.review.is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!root.join("FORBIDDEN_HELPER").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("code.rs")).unwrap(),
        "pub fn value() -> u32 { 2 }\n"
    );
    model.abort();
}

#[tokio::test]
async fn exec_and_stdin_manage_incremental_output_eof_and_completed_sessions() {
    use kyler_core::tools::{exec_command::WriteStdinArgs, shell::RunCommandArgs};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let env = environment(
        &root,
        &root,
        SandboxPolicy::workspace_write("exec-stdin", &root),
    );
    let manager = ProcessManager::default();
    let command = |text: &str, wait: u32| {
        serde_json::from_value::<RunCommandArgs>(
            json!({"program":"/bin/sh","args":["-c",text],"yield_time_ms":wait}),
        )
        .unwrap()
    };
    let follow = |id: &str, chars: &str, wait: u32| {
        serde_json::from_value::<WriteStdinArgs>(
            json!({"session_id":id,"chars":chars,"yield_time_ms":wait}),
        )
        .unwrap()
    };
    let quick = manager
        .exec_command(
            "owner",
            (*env).clone(),
            command("printf done; exit 7", 5000),
        )
        .await
        .unwrap();
    assert_eq!(quick["status"], "completed");
    assert_eq!(quick["exit_code"], 7);
    assert_eq!(quick["output"], "done");
    assert!(quick["session_id"].is_null());
    let text = "printf ready; read value; printf 'received:%s' \"$value\"";
    let running = manager
        .exec_command("owner", (*env).clone(), command(text, 1000))
        .await
        .unwrap();
    assert_eq!(running["output"], "ready");
    let id = running["session_id"].as_str().unwrap();
    assert!(
        manager
            .continue_command("other", follow(id, "", 0))
            .await
            .is_err()
    );
    let duplicate = manager
        .exec_command("owner", (*env).clone(), command(text, 0))
        .await
        .unwrap_err();
    assert!(duplicate.to_string().contains(id));
    let empty = manager
        .continue_command("owner", follow(id, "", 20))
        .await
        .unwrap();
    assert_eq!(empty["output"], "");
    assert_eq!(empty["input_sent"], false);
    assert_eq!(empty["status"], "running");
    let mut input = follow(id, "hello\n", 5000);
    input.close_stdin = true;
    let mut done = manager.continue_command("owner", input).await.unwrap();
    let mut output = done["output"].as_str().unwrap().to_owned();
    while done["status"] == "running" {
        done = manager
            .continue_command("owner", follow(id, "", 5000))
            .await
            .unwrap();
        output.push_str(done["output"].as_str().unwrap());
    }
    assert_eq!(output, "received:hello");
    assert_eq!(done["exit_code"], 0);
    let ended = manager
        .continue_command("owner", follow(id, "more", 0))
        .await
        .unwrap();
    assert_eq!(ended["input_sent"], false);
    assert!(ended["input_error"].as_str().unwrap().contains("ended"));
    assert_eq!(ended["output"], "");
    let eof = manager
        .exec_command("owner", (*env).clone(), command("cat; printf eof", 20))
        .await
        .unwrap();
    let mut input = follow(eof["session_id"].as_str().unwrap(), "body", 5000);
    input.close_stdin = true;
    let mut done = manager.continue_command("owner", input).await.unwrap();
    let mut output = done["output"].as_str().unwrap().to_owned();
    while done["status"] == "running" {
        done = manager
            .continue_command(
                "owner",
                follow(eof["session_id"].as_str().unwrap(), "", 5000),
            )
            .await
            .unwrap();
        output.push_str(done["output"].as_str().unwrap());
    }
    assert_eq!(output, "bodyeof");
    manager.shutdown();
}

#[tokio::test]
async fn stdin_reads_serialize_and_termination_stops_side_effects() {
    use kyler_core::tools::{exec_command::WriteStdinArgs, shell::RunCommandArgs};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let env = environment(
        &root,
        &root,
        SandboxPolicy::workspace_write("stdin-concurrency", &root),
    );
    let manager = ProcessManager::default();
    let command = |text: &str| {
        serde_json::from_value::<RunCommandArgs>(
            json!({"program":"/bin/sh","args":["-c",text],"yield_time_ms":0}),
        )
        .unwrap()
    };
    let follow = |id: &str| {
        serde_json::from_value::<WriteStdinArgs>(json!({"session_id":id,"yield_time_ms":5000}))
            .unwrap()
    };
    let running = manager
        .exec_command("owner", (*env).clone(), command("sleep 0.2; printf later"))
        .await
        .unwrap();
    let id = running["session_id"].as_str().unwrap();
    let (a, b) = tokio::join!(
        manager.continue_command("owner", follow(id)),
        manager.continue_command("owner", follow(id))
    );
    let combined = format!(
        "{}{}",
        a.unwrap()["output"].as_str().unwrap(),
        b.unwrap()["output"].as_str().unwrap()
    );
    assert_eq!(combined, "later");
    let running = manager
        .exec_command(
            "owner",
            (*env).clone(),
            command("sleep 1; touch must-not-run"),
        )
        .await
        .unwrap();
    let mut stop = follow(running["session_id"].as_str().unwrap());
    stop.terminate = true;
    let stopped = manager.continue_command("owner", stop).await.unwrap();
    assert_eq!(stopped["status"], "cancelled");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(!root.join("must-not-run").exists());
    manager.shutdown();
}

#[tokio::test]
async fn shell_waits_returns_tail_output_times_out_and_cleans_private_sessions() {
    use kyler_core::{
        context::ExecutionContext,
        tools::{Tool, ToolContext, shell::ShellTool},
    };
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let env = environment(
        &root,
        &root,
        SandboxPolicy::workspace_write("one-shot-shell", &root),
    );
    let manager = ProcessManager::default();
    let mut execution = ExecutionContext::new();
    let mut state = std::collections::HashMap::new();
    let mut context = ToolContext {
        processes: Some(&manager),
        execution: &mut execution,
        state: &mut state,
        session_id: "owner",
        tool_call_id: "shell",
        subagents: None,
        environment: Some(env.as_ref()),
    };
    let output = ShellTool
        .execute(
            r#"{"command":["/bin/sh","-c","printf done; printf warning >&2; exit 7"]}"#,
            &mut context,
        )
        .await
        .unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["status"], "completed");
    assert_eq!(value["exit_code"], 7);
    assert_eq!(value["stdout"], "done");
    assert_eq!(value["stderr"], "warning");
    assert!(value.get("session_id").is_none());
    assert!(manager.list("owner").is_empty());
    let eof = ShellTool
        .execute(
            r#"{"command":["/bin/cat"],"timeout_ms":5000}"#,
            &mut context,
        )
        .await
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&eof).unwrap()["exit_code"], 0);
    let output = ShellTool.execute(r#"{"command":["/bin/sh","-c","printf partial; sleep 2; touch must-not-run-shell"],"timeout_ms":500}"#,&mut context).await.unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["status"], "timed_out");
    assert_eq!(value["stdout"], "partial");
    assert!(manager.list("owner").is_empty());
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert!(!root.join("must-not-run-shell").exists());
    let future = ShellTool.execute(
        r#"{"command":["/bin/sh","-c","sleep 1; touch cancelled-shell"]}"#,
        &mut context,
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), future)
            .await
            .is_err()
    );
    assert!(manager.list("owner").is_empty());
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(!root.join("cancelled-shell").exists());
}
