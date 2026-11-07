use super::{Sandbox, SandboxRequest, WorkerResponse, policy::NetworkAccess};

use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use mxc_sdk::v1::{
    self, ContainerRequest, Containment, MxcProcess, NetworkAction, NetworkEgressPolicy,
    NetworkIngressPolicy, NetworkPolicy, NetworkRuntimeConfig,
};

pub struct MxcSandbox {
    _private: (),
}
impl MxcSandbox {
    pub fn new() -> Result<Self> {
        let support = v1::platform_support();
        ensure!(
            support.is_supported,
            "MXC backend unavailable: {}",
            support.reason.unwrap_or_default()
        );
        ensure!(
            cfg!(any(target_os = "macos", target_os = "linux")),
            "unsupported MXC host"
        );
        Ok(Self { _private: () })
    }
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
struct ProcessGuard(MxcProcess);
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}
#[async_trait::async_trait]
impl Sandbox for MxcSandbox {
    async fn run(&self, request: SandboxRequest) -> Result<WorkerResponse> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let _guard = CancelOnDrop(cancelled.clone());
        // Dropping this future signals the supervisor; it still owns and terminates
        // the native process rather than leaving a detached spawn_blocking workload.
        tokio::task::spawn_blocking(move || supervise(request, cancelled))
            .await
            .context("MXC supervisor panicked")?
    }
}
fn quote(value: &str) -> Result<String> {
    ensure!(!value.contains('\0'), "NUL in worker executable path");
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}
fn containment() -> Result<Containment> {
    #[cfg(target_os = "macos")]
    {
        let mut config = v1::configs::SeatbeltConfig::default();
        config.nested_pty = false;
        Ok(Containment::Seatbelt(config))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Containment::Bubblewrap)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        anyhow::bail!("Kyler's MXC adapter currently supports macOS and Linux only")
    }
}

fn bounded_read(
    mut stream: Box<dyn Read + Send>,
    limit: usize,
    total: Arc<AtomicUsize>,
    overflow: Arc<AtomicBool>,
    line: bool,
) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let n = stream.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        let end = if line {
            buffer[..n]
                .iter()
                .position(|&c| c == b'\n')
                .map_or(n, |i| i + 1)
        } else {
            n
        };
        let before = total.fetch_add(end, Ordering::SeqCst);
        if before.saturating_add(end) > limit {
            overflow.store(true, Ordering::SeqCst);
            return Err(std::io::Error::other("sandbox output limit exceeded"));
        }
        output.extend_from_slice(&buffer[..end]);
        if line && output.last() == Some(&b'\n') {
            break;
        }
    }
    Ok(output)
}
fn terminate(process: &mut MxcProcess) -> std::io::Result<()> {
    // Seatbelt's native session sweep can race a short-lived process while
    // reading its audit identity. Retry the SDK operation, never ignore an
    // unconfirmed cleanup or replace it with an unrestricted execution path.
    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        match process.kill() {
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Interrupted
                ) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            result => return result,
        }
    }
}

fn supervise(mut request: SandboxRequest, cancel: Arc<AtomicBool>) -> Result<WorkerResponse> {
    ensure!(
        request.policy.mode.isolated(),
        "MXC cannot execute an unrestricted sandbox policy"
    );
    let payload = serde_json::to_vec(&request.worker_request)?;
    ensure!(
        payload.len() <= request.policy.process.max_request_bytes,
        "sandbox worker request exceeds byte limit"
    );
    ensure!(
        !cancel.load(Ordering::SeqCst),
        "sandbox execution cancelled before spawn"
    );
    let mut readonly = request.policy.filesystem.readonly_paths.clone();
    readonly.extend(request.policy.runtime_readonly_paths.clone());
    readonly.push(request.worker_path.clone());
    let mut writable = request.policy.filesystem.readwrite_paths.clone();
    if request.policy.mode == super::SandboxMode::ReadOnly {
        readonly.push(request.temporary_directory.path().canonicalize()?);
    } else {
        writable.push(request.temporary_directory.path().canonicalize()?);
    }
    let string_paths = |paths: Vec<PathBuf>| -> Result<Vec<String>> {
        paths
            .into_iter()
            .map(|p| {
                p.into_os_string()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("sandbox paths must be UTF-8"))
            })
            .collect()
    };
    let network = match &request.policy.network {
        NetworkAccess::Deny => NetworkPolicy::default(),
        NetworkAccess::AllowAll | NetworkAccess::Inherit => {
            anyhow::bail!("unrestricted network policies require a direct worker")
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
    let network = NetworkPolicy {
        ingress: Some(NetworkIngressPolicy {
            default: Some(NetworkAction::Deny),
            host_loopback: Some(NetworkAction::Deny),
        }),
        ..network
    };
    let mut launch = ContainerRequest::new(format!(
        "exec {}",
        quote(
            request
                .worker_path
                .to_str()
                .context("worker path must be UTF-8")?
        )?
    ));
    launch.containment = containment()?;
    launch.filesystem = Some(v1::FilesystemPolicy {
        readonly_paths: string_paths(readonly)?,
        readwrite_paths: string_paths(writable)?,
        denied_paths: string_paths(request.policy.filesystem.denied_paths.clone())?,
        ..Default::default()
    });
    launch.network = Some(network);
    launch.ui = Some(Default::default());
    launch.timeout_ms = Some(request.policy.process.timeout_ms);
    launch.working_directory = Some(
        request
            .worker_request
            .environment
            .working_directory
            .to_string_lossy()
            .into_owned(),
    );
    launch.environment = Some(request.environment.into_iter().collect());
    launch.inherit_default_environment = Some(false);
    let mut process = ProcessGuard(
        v1::spawn(launch, Default::default()).context("MXC refused isolated worker launch")?,
    );
    for warning in process.0.warnings() {
        tracing::warn!(%warning,"MXC backend warning");
    }
    let stdout_close = process.0.stdout_closer();
    let stderr_close = process.0.stderr_closer();
    let mut stdin = process.0.take_stdin().context("MXC stdin missing")?;
    let stdout = process.0.take_stdout().context("MXC stdout missing")?;
    let stderr = process.0.take_stderr().context("MXC stderr missing")?;
    let writer = std::thread::spawn(move || -> std::io::Result<Box<dyn Write + Send>> {
        stdin.write_all(&payload)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(stdin)
    });
    let total = Arc::new(AtomicUsize::new(0));
    let overflow = Arc::new(AtomicBool::new(false));
    let cap = request.policy.process.max_output_bytes;
    let (tx, rx) = mpsc::sync_channel(1);
    let bytes = total.clone();
    let flag = overflow.clone();
    let progress = request.progress.take();
    let reader = std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let result = (|| -> std::io::Result<Vec<u8>> {
            loop {
                let mut line = Vec::new();
                reader
                    .by_ref()
                    .take(cap as u64 + 1)
                    .read_until(b'\n', &mut line)?;
                if line.is_empty() {
                    return Err(std::io::Error::other("Worker output closed"));
                }
                if bytes
                    .fetch_add(line.len(), Ordering::SeqCst)
                    .saturating_add(line.len())
                    > cap
                {
                    flag.store(true, Ordering::SeqCst);
                    return Err(std::io::Error::other("Sandbox output limit exceeded"));
                }
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line) {
                    if let Some(chunk) = value.get("progress") {
                        if let (Some(sender), Ok(chunk)) =
                            (&progress, serde_json::from_value(chunk.clone()))
                        {
                            let _ = sender.send(chunk);
                        }
                        continue;
                    }
                }
                return Ok(line);
            }
        })();
        let _ = tx.send(result);
    });
    let bytes = total.clone();
    let flag = overflow.clone();
    let errors = std::thread::spawn(move || bounded_read(stderr, cap, bytes, flag, false));
    let mut written = writer
        .join()
        .map_err(|_| anyhow::anyhow!("Sandbox stdin writer panicked"))??;
    let started = Instant::now();
    let result: Result<Vec<u8>> = loop {
        if let Some(input) = &request.input {
            for message in input.try_iter().take(16) {
                if serde_json::to_writer(&mut written, &message)
                    .and_then(|_| Ok(()))
                    .is_err()
                    || written
                        .write_all(b"\n")
                        .and_then(|_| written.flush())
                        .is_err()
                {
                    break;
                }
            }
        }
        if cancel.load(Ordering::SeqCst) {
            break Err(anyhow::anyhow!("sandbox execution cancelled"));
        }
        if overflow.load(Ordering::SeqCst) {
            break Err(anyhow::anyhow!("sandbox output limit exceeded"));
        }
        if started.elapsed() >= Duration::from_millis(request.policy.process.timeout_ms as u64) {
            break Err(anyhow::anyhow!("sandbox execution timed out"));
        }
        match rx.try_recv() {
            Ok(bytes) => break bytes.map_err(Into::into),
            Err(mpsc::TryRecvError::Disconnected) => {
                break Err(anyhow::anyhow!("sandbox stdout reader failed"));
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        match process.0.try_wait() {
            Ok(Some(code)) => {
                break Err(anyhow::anyhow!(
                    "sandbox worker exited before completing protocol (code {code})"
                ));
            }
            Err(e) => break Err(e.into()),
            Ok(None) => {}
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // The worker intentionally remains alive after writing its response. Killing
    // its process group here also cleans up ordinary background command children.
    let kill = terminate(&mut process.0);
    let wait = process.0.wait();
    if let Err(error) = &kill {
        tracing::error!(%error,"isolated worker termination failed");
    }
    if let Err(error) = &wait {
        tracing::error!(%error,"isolated worker reap failed");
    }
    if let Some(closer) = stdout_close {
        closer.close();
    }
    if let Some(closer) = stderr_close {
        closer.close();
    }
    let _ = reader.join();
    let stderr = errors
        .join()
        .map_err(|_| anyhow::anyhow!("sandbox stderr reader panicked"))?;
    let bytes = result.with_context(|| {
        format!(
            "worker stderr: {}",
            stderr
                .as_ref()
                .map(|v| String::from_utf8_lossy(v).into_owned())
                .unwrap_or_default()
        )
    })?;
    kill.context("could not terminate isolated worker")?;
    wait.context("could not reap isolated worker")?;
    drop(written);
    ensure!(
        !overflow.load(Ordering::SeqCst),
        "sandbox output limit exceeded"
    );
    serde_json::from_slice(&bytes).context("invalid sandbox worker response")
}
