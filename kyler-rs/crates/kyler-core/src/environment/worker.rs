//! Isolated worker request preparation and execution.
use super::ExecutionEnvironment;
use crate::sandbox::{ProcessChunk, ProcessInput, SandboxMode, SandboxRequest, WorkerRequest};

use std::{collections::BTreeMap, path::PathBuf, sync::mpsc::Receiver};

use anyhow::{Context, Result, ensure};
use tokio::sync::mpsc::UnboundedSender;

impl ExecutionEnvironment {
    pub async fn run_tool(&self, tool: &str, args_json: &str) -> Result<String> {
        self.run_tool_io(tool, args_json, None, None).await
    }
    pub async fn run_tool_io(
        &self,
        tool: &str,
        args_json: &str,
        progress: Option<UnboundedSender<ProcessChunk>>,
        input: Option<Receiver<ProcessInput>>,
    ) -> Result<String> {
        ensure!(
            matches!(
                tool,
                "review_git"
                    | "read_file"
                    | "list_dir"
                    | "grep_files"
                    | "apply_patch"
                    | "run_command"
                    | "read_attachment_document"
            ),
            "tool has no isolated worker implementation: {tool}"
        );
        if tool == "review_git" {
            ensure!(
                self.policy.mode == SandboxMode::ReadOnly
                    && matches!(self.policy.network, crate::sandbox::NetworkAccess::Deny),
                "Review Git requires a read-only, offline environment"
            );
        }
        if tool == "run_command" {
            ensure!(
                self.policy.process.allow_commands,
                "sandbox process policy denies command execution"
            );
        }
        let home = if self.policy.mode.isolated() {
            self.temporary_directory.path().to_path_buf()
        } else {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| self.temporary_directory.path().to_path_buf())
        };
        let mut environment = BTreeMap::from([
            ("KYLER_INTERNAL_TOOL_WORKER".into(), "1".into()),
            ("PATH".into(), self.policy.executable_search_path.clone()),
            ("LANG".into(), "C.UTF-8".into()),
            (
                "TMPDIR".into(),
                self.temporary_directory
                    .path()
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("HOME".into(), home.to_string_lossy().into_owned()),
        ]);
        if let Some(rustup) = &self.policy.rustup {
            environment.insert(
                "RUSTUP_HOME".into(),
                rustup.home.to_string_lossy().into_owned(),
            );
            environment.insert("RUSTUP_AUTO_INSTALL".into(), "0".into());
            if let Some(toolchain) = &rustup.toolchain {
                environment.insert("RUSTUP_TOOLCHAIN".into(), toolchain.clone());
            }
            if self.policy.mode.isolated() {
                environment.insert(
                    "CARGO_HOME".into(),
                    self.temporary_directory
                        .path()
                        .join(".cargo")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        let mut filesystem = self.policy.filesystem.clone();
        if self.policy.mode == SandboxMode::ReadOnly {
            filesystem
                .readonly_paths
                .push(self.temporary_directory.path().canonicalize()?);
        } else {
            filesystem
                .readwrite_paths
                .push(self.temporary_directory.path().canonicalize()?);
        }
        let mut policy = self.policy.clone();
        if progress.is_some() {
            policy.process.timeout_ms = policy.process.background_timeout_ms;
        }
        let arguments: serde_json::Value = serde_json::from_str(args_json)?;
        if tool == "run_command" {
            let command: crate::tools::command::RunCommandArgs =
                serde_json::from_value(arguments.clone())?;
            command.validate()?;
            command.directory(&self.snapshot.working_directory, &filesystem)?;
            if let Some(timeout) = command.timeout_ms {
                ensure!(
                    timeout <= policy.process.timeout_ms,
                    "requested command timeout exceeds host limit of {} ms",
                    policy.process.timeout_ms
                );
                policy.process.timeout_ms = timeout;
            }
        }
        let request = SandboxRequest {
            input,
            worker_request: WorkerRequest {
                stream_output: progress.is_some(),
                tool: tool.into(),
                arguments,
                environment: self.snapshot.clone(),
                filesystem,
                allow_commands: self.policy.process.allow_commands,
                max_output_bytes: self.policy.process.max_output_bytes,
            },
            progress,
            worker_path: self.worker_path.clone(),
            policy,
            environment,
            temporary_directory: self.temporary_directory.clone(),
        };
        let response = self.backend.run(request).await?;
        if let Some(error) = response.error {
            anyhow::bail!("sandbox tool failed: {error}");
        }
        response.output.context("sandbox worker returned no output")
    }
}

#[cfg(test)]
mod tests {
    use super::super::ExecutionEnvironment;
    use crate::sandbox::{
        NetworkAccess, Sandbox, SandboxPolicy, SandboxRequest, WorkerResponse,
        policy::RustupRuntime,
    };

    use std::{
        path::{Path, PathBuf},
        sync::Arc,
    };

    use anyhow::Result;

    struct RuntimeBackend {
        rustup_home: PathBuf,
        workspace: PathBuf,
    }
    #[async_trait::async_trait]
    impl Sandbox for RuntimeBackend {
        async fn run(&self, request: SandboxRequest) -> Result<WorkerResponse> {
            assert_eq!(
                Path::new(&request.environment["RUSTUP_HOME"]),
                self.rustup_home
            );
            assert_eq!(request.environment["RUSTUP_AUTO_INSTALL"], "0");
            let home = Path::new(&request.environment["HOME"]);
            assert_ne!(home, self.rustup_home);
            assert!(Path::new(&request.environment["CARGO_HOME"]).starts_with(home));
            assert_eq!(
                request.worker_request.filesystem.readwrite_paths,
                [self.workspace.clone(), home.canonicalize().unwrap()]
            );
            assert!(!request.environment.contains_key("KYLER_LLM_API_KEY"));
            assert!(matches!(request.policy.network, NetworkAccess::Deny));
            Ok(WorkerResponse {
                output: Some("configured runtime".into()),
                error: None,
            })
        }
    }
    #[tokio::test]
    async fn rustup_runtime_keeps_private_home_and_never_inherits_host_credentials() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let runtime_home = runtime.path().canonicalize().unwrap();
        let binary = std::env::current_exe().unwrap().canonicalize().unwrap();
        let mut policy = SandboxPolicy::workspace_write("rust-runtime", &workspace);
        policy.runtime_readonly_paths = vec![runtime_home.clone(), binary.clone()];
        policy.rustup = Some(RustupRuntime {
            home: runtime_home.clone(),
            binary,
            toolchain: None,
        });
        let environment = ExecutionEnvironment::new(
            &workspace,
            &workspace,
            policy,
            std::env::current_exe().unwrap(),
            Arc::new(RuntimeBackend {
                rustup_home: runtime_home,
                workspace: workspace.clone(),
            }),
        )
        .unwrap();
        assert_eq!(
            environment
                .run_tool("run_command", r#"{"program":"cargo","args":["--version"]}"#)
                .await
                .unwrap(),
            "configured runtime"
        );
    }
}
