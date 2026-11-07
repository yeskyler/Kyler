//! Explicit unrestricted/external execution, with bounded I/O and owned process lifetime.
use super::{Sandbox, SandboxRequest, WorkerResponse};

use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::oneshot::Receiver,
};

pub struct DirectSandbox;
pub struct RestrictingSandbox;
#[async_trait::async_trait]
impl Sandbox for RestrictingSandbox {
    async fn run(&self, request: SandboxRequest) -> Result<WorkerResponse> {
        super::MxcSandbox::new()?.run(request).await
    }
}

pub(crate) struct ChildGroup {
    pub child: Child,
    id: i32,
    active: bool,
}
impl ChildGroup {
    pub fn spawn(mut command: Command) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;

            command.as_std_mut().process_group(0);
        }
        let child = command.kill_on_drop(true).spawn()?;
        let id = child.id().context("worker process has no ID")? as i32;
        Ok(Self {
            child,
            id,
            active: true,
        })
    }
    fn kill_group(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-self.id, libc::SIGKILL);
        }
        let _ = self.child.start_kill();
    }
    pub async fn stop(&mut self) -> Result<()> {
        self.kill_group();
        self.child.wait().await?;
        self.active = false;
        Ok(())
    }
}
impl Drop for ChildGroup {
    fn drop(&mut self) {
        if self.active {
            self.kill_group();
        }
    }
}

#[async_trait::async_trait]
impl Sandbox for DirectSandbox {
    async fn run(&self, request: SandboxRequest) -> Result<WorkerResponse> {
        ensure!(
            !request.policy.mode.isolated(),
            "direct execution requires an explicit unrestricted or external sandbox mode"
        );
        let (cancel, stopped) = tokio::sync::oneshot::channel::<()>();
        // Cancellation drops the sender; the independent supervisor still kills and reaps the child.
        let task = tokio::spawn(supervise(request, stopped));
        let result = task.await.context("direct worker supervisor panicked")?;
        drop(cancel);
        result
    }
}
async fn supervise(
    mut request: SandboxRequest,
    mut stopped: Receiver<()>,
) -> Result<WorkerResponse> {
    let mut payload = serde_json::to_vec(&request.worker_request)?;
    ensure!(
        payload.len() <= request.policy.process.max_request_bytes,
        "worker request exceeds byte limit"
    );
    payload.push(b'\n');
    let mut command = Command::new(&request.worker_path);
    command
        .current_dir(&request.worker_request.environment.working_directory)
        .env_clear()
        .envs(&request.environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut process = ChildGroup::spawn(command)?;
    let mut stdin = process.child.stdin.take().context("worker stdin missing")?;
    let stdout = process
        .child
        .stdout
        .take()
        .context("worker stdout missing")?;
    let mut stderr = process
        .child
        .stderr
        .take()
        .context("worker stderr missing")?;
    let total = Arc::new(AtomicUsize::new(0));
    let cap = request.policy.process.max_output_bytes;
    let count = total.clone();
    let errors = tokio::spawn(async move {
        let mut buffer = [0u8; 4096];
        loop {
            let n = stderr.read(&mut buffer).await?;
            if n == 0 {
                return Ok::<_, std::io::Error>(());
            }
            if count.fetch_add(n, Ordering::SeqCst).saturating_add(n) > cap {
                return Err(std::io::Error::other("worker output limit exceeded"));
            }
        }
    });
    let input = request.input.take();
    let progress = request.progress.take();
    let read = async move {
        stdin.write_all(&payload).await?;
        stdin.flush().await?;
        let mut output = stdout;
        let mut pending = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let messages = input
                .as_ref()
                .map(|input| input.try_iter().take(16).collect::<Vec<_>>())
                .unwrap_or_default();
            for message in messages {
                stdin.write_all(&serde_json::to_vec(&message)?).await?;
                stdin.write_all(b"\n").await?;
                stdin.flush().await?;
            }
            ensure!(
                total.load(Ordering::SeqCst) <= cap,
                "worker output limit exceeded"
            );
            let n = tokio::select! {
                read = output.read(&mut buffer) => read?,
                _ = tokio::time::sleep(Duration::from_millis(20)) => continue,
            };
            ensure!(n > 0, "worker output closed before a response");
            ensure!(
                total.fetch_add(n, Ordering::SeqCst).saturating_add(n) <= cap,
                "worker output limit exceeded"
            );
            pending.extend_from_slice(&buffer[..n]);
            while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                let line = pending.drain(..=end).collect::<Vec<_>>();
                let value: serde_json::Value =
                    serde_json::from_slice(&line).context("invalid worker response")?;
                if let Some(chunk) = value.get("progress") {
                    if let Some(sender) = &progress {
                        let _ = sender.send(serde_json::from_value(chunk.clone())?);
                    }
                } else {
                    return serde_json::from_value(value).context("invalid worker result");
                }
            }
        }
    };
    let result = tokio::select! {
        result = tokio::time::timeout(Duration::from_millis(request.policy.process.timeout_ms as u64), read) => result.unwrap_or_else(|_| Err(anyhow::anyhow!("worker execution timed out"))),
        _ = &mut stopped => Err(anyhow::anyhow!("worker execution cancelled")),
    };
    process.stop().await?;
    match tokio::time::timeout(Duration::from_secs(1), errors).await {
        Ok(result) => result.context("worker stderr reader panicked")??,
        Err(_) => anyhow::bail!("worker stderr did not close"),
    }
    result
}
