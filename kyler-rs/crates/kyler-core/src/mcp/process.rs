//! Long-lived stdio bridge. The supervisor owns and reaps the native sandbox process.
use crate::{environment::ExecutionEnvironment, sandbox::NetworkAccess};

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use mxc_sdk::v1::{
    self, ContainerRequest, Containment, NetworkAction, NetworkEgressPolicy, NetworkIngressPolicy,
    NetworkPolicy, NetworkRuntimeConfig,
};

pub struct ProcessLease(Arc<AtomicBool>);
impl ProcessLease {
    pub(super) fn stop(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
impl Drop for ProcessLease {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[cfg(unix)]
pub async fn launch(
    command: &str,
    args: &[String],
    variables: BTreeMap<String, String>,
    environment: Arc<ExecutionEnvironment>,
    package_root: Option<&std::path::Path>,
) -> Result<(tokio::net::UnixStream, ProcessLease)> {
    ensure!(
        environment.policy().process.allow_commands,
        "sandbox policy denies MCP process execution"
    );
    let executable = std::path::Path::new(command)
        .canonicalize()
        .context("MCP executable is unavailable")?;
    if !environment.policy().mode.isolated() {
        return launch_direct(&executable, args, variables, environment).await;
    }
    let quote = |value: &str| -> Result<String> {
        ensure!(!value.contains('\0'), "NUL in MCP command");
        Ok(format!("'{}'", value.replace('\'', "'\\''")))
    };
    let command = std::iter::once(executable.to_string_lossy().into_owned())
        .chain(args.iter().cloned())
        .map(|v| quote(&v))
        .collect::<Result<Vec<_>>>()?
        .join(" ");
    let policy = environment.policy();
    let paths = |values: Vec<std::path::PathBuf>| {
        values
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    let mut readonly = policy.filesystem.readonly_paths.clone();
    readonly.extend(policy.runtime_readonly_paths.clone());
    readonly.push(executable);
    if let Some(directory) = package_root {
        readonly.push(directory.to_path_buf());
    }
    let mut request = ContainerRequest::new(format!("exec {command}"));
    #[cfg(target_os = "macos")]
    {
        let mut config = v1::configs::SeatbeltConfig::default();
        config.nested_pty = false;
        request.containment = Containment::Seatbelt(config);
    }
    #[cfg(target_os = "linux")]
    {
        request.containment = Containment::Bubblewrap;
    }
    request.filesystem = Some(v1::FilesystemPolicy {
        readonly_paths: paths(readonly),
        readwrite_paths: paths(policy.filesystem.readwrite_paths.clone()),
        denied_paths: paths(policy.filesystem.denied_paths.clone()),
        ..Default::default()
    });
    let network = match &policy.network {
        NetworkAccess::Deny => NetworkPolicy::default(),
        NetworkAccess::AllowAll | NetworkAccess::Inherit => {
            anyhow::bail!("unrestricted MCP transport requires a direct process")
        }
        NetworkAccess::AllowOutbound => NetworkPolicy {
            egress: Some(NetworkEgressPolicy {
                default: Some(NetworkAction::Allow),
                ..Default::default()
            }),
            ..Default::default()
        },
        NetworkAccess::Proxy { url } => NetworkPolicy {
            runtime_config: Some(NetworkRuntimeConfig {
                network_proxy: Some(url.clone()),
            }),
            ..Default::default()
        },
    };
    request.network = Some(NetworkPolicy {
        ingress: Some(NetworkIngressPolicy {
            default: Some(NetworkAction::Deny),
            host_loopback: Some(NetworkAction::Deny),
        }),
        ..network
    });
    request.ui = Some(Default::default());
    request.working_directory = Some(
        environment
            .session_environment()
            .working_directory
            .to_string_lossy()
            .into_owned(),
    );
    let mut vars = BTreeMap::from([
        ("PATH".into(), policy.executable_search_path.clone()),
        ("LANG".into(), "C.UTF-8".into()),
    ]);
    vars.extend(variables);
    request.environment = Some(vars.into_iter().collect());
    request.inherit_default_environment = Some(false);
    let (socket, bridge) = std::os::unix::net::UnixStream::pair()?;
    socket.set_nonblocking(true)?;
    let socket = tokio::net::UnixStream::from_std(socket)?;
    let stop = Arc::new(AtomicBool::new(false));
    let lease = ProcessLease(stop.clone());
    let (ready, started) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let run = || -> Result<()> {
            let process =
                v1::spawn(request, Default::default()).context("MCP sandbox launch failed")?;
            struct Guard(v1::MxcProcess);
            impl Drop for Guard {
                fn drop(&mut self) {
                    let _ = self.0.kill();
                    let _ = self.0.wait();
                }
            }
            let mut process = Guard(process);
            let mut stdin = process.0.take_stdin().context("MCP stdin missing")?;
            let mut stdout = process.0.take_stdout().context("MCP stdout missing")?;
            let mut stderr = process.0.take_stderr().context("MCP stderr missing")?;
            let stdout_close = process.0.stdout_closer();
            let stderr_close = process.0.stderr_closer();
            let mut incoming = bridge.try_clone()?;
            let mut outgoing = bridge.try_clone()?;
            let write = std::thread::spawn(move || {
                let _ = std::io::copy(&mut incoming, &mut stdin);
                let _ = stdin.flush();
            });
            let output_stop = stop.clone();
            let read = std::thread::spawn(move || {
                let mut buffer = [0; 8192];
                let mut line_bytes = 0usize;
                while let Ok(n) = stdout.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    for byte in &buffer[..n] {
                        line_bytes = if *byte == b'\n' { 0 } else { line_bytes + 1 };
                        if line_bytes > 8 * 1024 * 1024 {
                            output_stop.store(true, Ordering::SeqCst);
                            let _ = outgoing.shutdown(std::net::Shutdown::Both);
                            return;
                        }
                    }
                    if outgoing.write_all(&buffer[..n]).is_err() {
                        break;
                    }
                }
                let _ = outgoing.shutdown(std::net::Shutdown::Both);
            });
            // Never forward server stderr to application logs (it may contain credentials).
            let errors = std::thread::spawn(move || {
                let mut bytes = [0; 4096];
                while matches!(stderr.read(&mut bytes), Ok(n) if n>0) {}
            });
            let _ = ready.send(());
            while !stop.load(Ordering::SeqCst) && matches!(process.0.try_wait(), Ok(None)) {
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = process.0.kill();
            let _ = process.0.wait();
            let _ = bridge.shutdown(std::net::Shutdown::Both);
            if let Some(close) = stdout_close {
                close.close();
            }
            if let Some(close) = stderr_close {
                close.close();
            }
            let _ = write.join();
            let _ = read.join();
            let _ = errors.join();
            Ok(())
        };
        let _ = run();
    });
    started
        .await
        .context("MCP sandbox process could not start")?;
    Ok((socket, lease))
}

#[cfg(unix)]
async fn launch_direct(
    executable: &std::path::Path,
    args: &[String],
    variables: BTreeMap<String, String>,
    environment: Arc<ExecutionEnvironment>,
) -> Result<(tokio::net::UnixStream, ProcessLease)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut command = tokio::process::Command::new(executable);
    command
        .args(args)
        .current_dir(&environment.session_environment().working_directory)
        .env_clear()
        .env("PATH", &environment.policy().executable_search_path)
        .env("LANG", "C.UTF-8")
        .envs(variables)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(home) = std::env::var_os("HOME") {
        command.env("HOME", home);
    }
    let mut process = crate::sandbox::direct::ChildGroup::spawn(command)?;
    let mut stdin = process.child.stdin.take().context("MCP stdin missing")?;
    let mut stdout = process.child.stdout.take().context("MCP stdout missing")?;
    let mut stderr = process.child.stderr.take().context("MCP stderr missing")?;
    let (socket, bridge) = tokio::net::UnixStream::pair()?;
    let stop = Arc::new(AtomicBool::new(false));
    let lease = ProcessLease(stop.clone());
    tokio::spawn(async move {
        let (mut incoming, mut outgoing) = bridge.into_split();
        let write = async { tokio::io::copy(&mut incoming, &mut stdin).await };
        let read = async {
            let mut buffer = [0u8; 8192];
            let mut line_bytes = 0usize;
            loop {
                let n = stdout.read(&mut buffer).await?;
                if n == 0 {
                    return Ok::<(), std::io::Error>(());
                }
                for byte in &buffer[..n] {
                    line_bytes = if *byte == b'\n' { 0 } else { line_bytes + 1 };
                    if line_bytes > 8 * 1024 * 1024 {
                        return Err(std::io::Error::other("MCP message exceeds byte limit"));
                    }
                }
                outgoing.write_all(&buffer[..n]).await?;
            }
        };
        let errors = async { tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await };
        let cancelled = async {
            while !stop.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::select! { _ = write => {}, _ = read => {}, _ = errors => {}, _ = cancelled => {} }
        let _ = process.stop().await;
    });
    Ok((socket, lease))
}
