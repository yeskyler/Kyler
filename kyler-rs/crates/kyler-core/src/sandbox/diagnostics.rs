//! Credential-free native checks using disposable fixtures and the production worker path.
use crate::{ExecutionEnvironment, NetworkAccess, SandboxPolicy};

use std::{net::SocketAddr, path::Path, time::Duration};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Serialize, Deserialize)]
pub struct SandboxCheck {
    pub mode: String,
    pub child_process: bool,
    pub ripgrep: bool,
    pub workspace_read_write: bool,
    pub readonly_enforced: bool,
    pub denied_read_enforced: bool,
    pub outside_workspace_enforced: bool,
    pub host_loopback_blocked: bool,
}

/// Exercise both isolated network policies used by Kyler. No public endpoints or user data.
pub async fn check(worker: &Path) -> Result<Vec<SandboxCheck>> {
    let directory = tempfile::Builder::new().prefix("kyler-doctor-").tempdir()?;
    let root = directory.path().canonicalize()?;
    let workspace = root.join("workspace");
    std::fs::create_dir(&workspace)?;
    std::fs::write(workspace.join("allowed"), "fixture")?;
    std::fs::write(workspace.join("readonly"), "fixture")?;
    std::fs::write(workspace.join("denied"), "fixture")?;
    let outside = root.join("outside");
    std::fs::write(&outside, "fixture")?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    // Prove the target is reachable from the host before asserting sandbox denial.
    let host = TcpStream::connect(address).await?;
    let (accepted, _) = listener.accept().await?;
    drop((host, accepted));
    let runtime = crate::environment::workspaces::runtime::Runtime::discover();
    let mut results = Vec::new();
    for (mode, network) in [
        ("offline", NetworkAccess::Deny),
        ("outbound", NetworkAccess::AllowOutbound),
    ] {
        let mut policy = SandboxPolicy::workspace_write("doctor", &workspace);
        policy.network = network;
        policy.executable_search_path = runtime.search_path();
        policy.runtime_readonly_paths = runtime.readonly_paths.clone();
        policy
            .filesystem
            .readonly_paths
            .push(workspace.join("readonly"));
        policy
            .filesystem
            .denied_paths
            .push(workspace.join("denied"));
        let environment = ExecutionEnvironment::mxc(&workspace, &workspace, policy, worker)
            .with_context(|| format!("{mode}: cannot initialize native sandbox"))?;
        let output = environment
            .run_tool(
                "sandbox_diagnostics",
                &json!({"outside":outside,"loopback":address}).to_string(),
            )
            .await
            .with_context(|| format!("{mode}: native sandbox execution failed"))?;
        let result: SandboxCheck = serde_json::from_str(&output)?;
        ensure!(result.ripgrep, "{mode}: sandbox ripgrep search failed");
        ensure!(
            result.child_process,
            "{mode}: child process execution failed"
        );
        ensure!(
            result.workspace_read_write,
            "{mode}: workspace read/write failed"
        );
        ensure!(
            result.readonly_enforced,
            "{mode}: read-only protection failed"
        );
        ensure!(
            result.denied_read_enforced,
            "{mode}: denied file was readable"
        );
        ensure!(
            result.outside_workspace_enforced,
            "{mode}: outside workspace access was allowed"
        );
        ensure!(
            result.host_loopback_blocked,
            "{mode}: host loopback was accessible"
        );
        ensure!(
            std::fs::read_to_string(workspace.join("readonly"))? == "fixture",
            "{mode}: protected fixture changed"
        );
        ensure!(
            std::fs::read_to_string(&outside)? == "fixture",
            "{mode}: outside fixture changed"
        );
        results.push(SandboxCheck {
            mode: mode.into(),
            ..result
        });
    }
    Ok(results)
}

/// Internal worker operation, deliberately using OS I/O rather than application path checks.
/// This operation is not registered as a model tool.
pub(super) async fn probe(arguments: Value) -> Result<String> {
    #[derive(Deserialize)]
    struct Arguments {
        outside: std::path::PathBuf,
        loopback: SocketAddr,
    }
    let args: Arguments = serde_json::from_value(arguments)?;
    ensure!(
        args.loopback.ip().is_loopback(),
        "diagnostic target must be loopback"
    );
    let child_process = tokio::process::Command::new("/bin/sh")
        .args(["-c", "printf kyler-doctor"])
        .output()
        .await
        .context("cannot start sandbox child process")?;
    let child_process = child_process.status.success() && child_process.stdout == b"kyler-doctor";
    let matches = crate::tools::grep_files::grep_files(crate::tools::grep_files::GrepFilesArgs {
        pattern: "^fixture$".into(),
        include: Some("allowed".into()),
        path: Some(".".into()),
        limit: 10,
    })
    .await
    .context(
        "sandbox ripgrep search failed; install ripgrep and ensure rg is on the service PATH",
    )?;
    let ripgrep = matches.lines().any(|line| {
        Path::new(line)
            .file_name()
            .is_some_and(|name| name == "allowed")
    });
    let workspace_read_write = std::fs::read_to_string("allowed")? == "fixture"
        && std::fs::write("written", "probe").is_ok()
        && std::fs::read_to_string("written")? == "probe";
    let readonly_enforced = std::fs::read_to_string("readonly")? == "fixture"
        && std::fs::OpenOptions::new()
            .write(true)
            .open("readonly")
            .is_err();
    let denied_read_enforced = std::fs::read("denied").is_err();
    let outside_workspace_enforced = std::fs::read(&args.outside).is_err()
        && std::fs::OpenOptions::new()
            .write(true)
            .open(&args.outside)
            .is_err();
    let host_loopback_blocked = !matches!(
        tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(args.loopback)).await,
        Ok(Ok(_))
    );
    Ok(serde_json::to_string(&SandboxCheck {
        mode: String::new(),
        child_process,
        ripgrep,
        workspace_read_write,
        readonly_enforced,
        denied_read_enforced,
        outside_workspace_enforced,
        host_loopback_blocked,
    })?)
}
