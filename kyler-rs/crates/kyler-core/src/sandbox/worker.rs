use super::{WorkerRequest, WorkerResponse};
use crate::{
    attachments::AttachmentStore,
    tools::{
        apply_patch::{ApplyPatchArgs, apply_patch_in, validate_patch_access},
        command::RunCommandArgs,
        grep_files::{GrepFilesArgs, grep_files},
        list_dir::{ListDirArgs, list_dir},
        read_file::{ReadFileArgs, read_file},
    },
};

use std::{
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, Result, ensure};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::mpsc::UnboundedReceiver,
};

use kyler_protocol::api::attachments::ReadAttachmentRequest;

pub async fn dispatch(request: WorkerRequest) -> WorkerResponse {
    dispatch_controlled(request, None).await
}
async fn dispatch_controlled(
    request: WorkerRequest,
    control: Option<UnboundedReceiver<super::ProcessInput>>,
) -> WorkerResponse {
    let result = execute(request, control).await;
    match result {
        Ok(output) => WorkerResponse {
            output: Some(output),
            error: None,
        },
        Err(error) => WorkerResponse {
            output: None,
            error: Some(format!("{error:#}")),
        },
    }
}
async fn execute(
    request: WorkerRequest,
    mut control: Option<UnboundedReceiver<super::ProcessInput>>,
) -> Result<String> {
    let cwd = &request.environment.working_directory;
    let fs = &request.filesystem;
    match request.tool.as_str() {
        "sandbox_diagnostics" => super::diagnostics::probe(request.arguments).await,
        "review_git" => {
            ensure!(
                !request.allow_commands && request.filesystem.readwrite_paths.is_empty(),
                "Review Git requires a read-only worker"
            );
            crate::subagent::review::git::execute(&request).await
        }
        "read_attachment_document" => {
            #[derive(serde::Deserialize)]
            struct Args {
                root: PathBuf,
                session: String,
                request: ReadAttachmentRequest,
            }
            let args: Args = serde_json::from_value(request.arguments)?;
            let root = fs.check_read(cwd, &args.root)?;
            let store = AttachmentStore::new(root);
            Ok(serde_json::to_string(
                &store.read_uncached(&args.session, &args.request)?,
            )?)
        }

        "read_file" => {
            let mut args: ReadFileArgs = serde_json::from_value(request.arguments)?;
            ensure!(
                args.limit <= 2000,
                "sandbox read_file limit cannot exceed 2000"
            );
            args.file_path = fs
                .check_read(cwd, Path::new(&args.file_path))?
                .to_string_lossy()
                .into_owned();
            let path = args.file_path.clone();
            Ok(format!("Absolute path: {path}\n{}", read_file(args).await?))
        }
        "list_dir" => {
            let mut args: ListDirArgs = serde_json::from_value(request.arguments)?;
            ensure!(
                args.limit <= 2000 && args.depth <= 16,
                "sandbox directory limit/depth too large"
            );
            args.dir_path = fs
                .check_read(cwd, Path::new(&args.dir_path))?
                .to_string_lossy()
                .into_owned();
            // The OS policy enforces traversal permissions for every nested entry.
            list_dir(args).await
        }
        "grep_files" => {
            let mut args: GrepFilesArgs = serde_json::from_value(request.arguments)?;
            let path = args.path.as_deref().unwrap_or(".");
            args.path = Some(
                fs.check_read(cwd, Path::new(path))?
                    .to_string_lossy()
                    .into_owned(),
            );
            grep_files(args).await
        }
        "apply_patch" => {
            let args: ApplyPatchArgs = serde_json::from_value(request.arguments)?;
            validate_patch_access(&request.environment.workspace_root, &args.input, fs)?;
            apply_patch_in(&request.environment.workspace_root, &args.input).await
        }
        "run_command" => {
            ensure!(
                request.allow_commands,
                "sandbox process policy denies command execution"
            );
            let args: RunCommandArgs = serde_json::from_value(request.arguments)?;
            args.validate()?;
            let cwd = args.directory(cwd, fs)?;
            let program = if Path::new(&args.program).is_relative()
                && Path::new(&args.program).components().count() > 1
            {
                cwd.join(&args.program)
            } else {
                PathBuf::from(&args.program)
            };
            let mut command = Command::new(&program);
            command
                .args(&args.args)
                .current_dir(&cwd)
                .env_remove("KYLER_INTERNAL_TOOL_WORKER")
                .envs(&args.environment)
                .stdin(if request.stream_output {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            let mut child = command.spawn().map_err(|error| {
                let detail = if error.kind() == std::io::ErrorKind::NotFound {
                    "not found in the sandbox PATH, or its interpreter/working directory is unavailable; check the sandbox runtime before concluding that the host tool is not installed"
                } else if error.kind() == std::io::ErrorKind::PermissionDenied {
                    "execution denied by filesystem permissions or sandbox policy"
                } else {
                    "process launch failed"
                };
                anyhow::Error::new(error).context(format!("could not start sandboxed command `{}`: {detail}", args.program))
            })?;
            let stdout = child.stdout.take().context("command stdout missing")?;
            let stderr = child.stderr.take().context("command stderr missing")?;
            // The outer MXC supervisor bounds lifetime and combined encoded output.
            let limit = request.max_output_bytes as u64;
            let stdin = child.stdin.take();
            let write_input = async move {
                if let (Some(mut input), Some(ref mut messages)) = (stdin, control.take()) {
                    while let Some(message) = messages.recv().await {
                        if input.write_all(message.data.as_bytes()).await.is_err() {
                            break;
                        }
                        let _ = input.flush().await;
                        if message.close {
                            break;
                        }
                    }
                }
            };
            let (out, err, status) = tokio::join!(
                read_output_stream(stdout, limit, request.stream_output.then_some("stdout")),
                read_output_stream(stderr, limit, request.stream_output.then_some("stderr")),
                async {
                    tokio::pin!(write_input);
                    tokio::select! {status=child.wait()=>status,_=&mut write_input=>child.wait().await}
                }
            );
            Ok(serde_json::json!({"exit_code":status?.code(),"working_directory":cwd,"program":program,"stdout":String::from_utf8_lossy(&out?),"stderr":String::from_utf8_lossy(&err?)}).to_string())
        }
        _ => anyhow::bail!("unsupported isolated tool: {}", request.tool),
    }
}
async fn read_output_stream(
    mut reader: impl AsyncRead + Unpin,
    limit: u64,
    stream: Option<&str>,
) -> Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut buffer = [0u8; 4096];
    let mut pending = Vec::new();
    loop {
        let n = reader.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        result.extend_from_slice(&buffer[..n]);
        ensure!(
            result.len() as u64 <= limit,
            "Sandbox command output limit exceeded"
        );
        if let Some(stream) = stream {
            pending.extend_from_slice(&buffer[..n]);
            let end = match std::str::from_utf8(&pending) {
                Ok(_) => pending.len(),
                Err(error) if error.error_len().is_none() => error.valid_up_to(),
                Err(_) => pending.len(),
            };
            if end > 0 {
                emit_progress(stream, &String::from_utf8_lossy(&pending[..end]))?;
                pending.drain(..end);
            }
        }
    }
    if let Some(stream) = stream {
        if !pending.is_empty() {
            emit_progress(stream, &String::from_utf8_lossy(&pending))?;
        }
    }
    Ok(result)
}
fn emit_progress(stream: &str, text: &str) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(
        &mut stdout,
        &serde_json::json!({"progress":{"stream":stream,"text":text}}),
    )?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

/// Serve one isolated worker request on stdin/stdout. Called only by a worker entry point.
pub async fn run_stdio() -> anyhow::Result<()> {
    // This binary is a tool workload, not an MXC CLI wrapper. The host launches it
    // using mxc_sdk::v1::spawn and supplies one newline-delimited JSON request.
    let mut input = Vec::new();
    std::io::stdin()
        .lock()
        .take(64 * 1024 * 1024 + 1)
        .read_until(b'\n', &mut input)?;
    let mut streamed = false;
    let response = if input.len() > 64 * 1024 * 1024 {
        WorkerResponse {
            output: None,
            error: Some("worker request exceeds 64 MiB".into()),
        }
    } else {
        match serde_json::from_slice::<WorkerRequest>(&input) {
            Ok(request) => {
                let request: WorkerRequest = request;
                streamed = request.stream_output;
                let control = if streamed {
                    let (send, receive) = tokio::sync::mpsc::unbounded_channel();
                    std::thread::spawn(move || {
                        let stdin = std::io::stdin();
                        for line in stdin.lock().lines() {
                            let Ok(line) = line else {
                                break;
                            };
                            let Ok(input) = serde_json::from_str::<super::ProcessInput>(&line)
                            else {
                                continue;
                            };
                            if send.send(input).is_err() {
                                break;
                            }
                        }
                    });
                    Some(receive)
                } else {
                    None
                };
                dispatch_controlled(request, control).await
            }
            Err(error) => WorkerResponse {
                output: None,
                error: Some(format!("invalid worker request: {error}")),
            },
        }
    };
    let encoded = serde_json::to_vec(&response)?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&encoded)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    drop(stdout);
    // Keep the process-group leader alive until the supervisor kills it. Otherwise
    // a successful shell command could leave background descendants behind.
    if streamed {
        std::future::pending::<()>().await;
    }
    let mut control = [0u8; 1];
    let _ = std::io::stdin().read(&mut control);
    Ok(())
}
