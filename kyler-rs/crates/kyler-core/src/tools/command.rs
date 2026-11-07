use crate::{
    approval::ToolOperation,
    processes::ProcessManager,
    sandbox::ProcessInput,
    tools::{
        ActivityKind, CompressedToolOutput, Tool, ToolContext, ToolExecutionMode, ToolOutputBudget,
        output::text_prefix,
    },
};

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct RunCommandArgs {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Per-call directory, relative to the session directory or absolute within readable roots.
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
    /// Child-process variables only. Does not modify the session or host environment.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Must not exceed the host-configured command or background timeout.
    #[serde(default)]
    pub timeout_ms: Option<u32>,
    /// Return a process ID if the command is still running after this wait.
    #[serde(default = "default_yield_time")]
    pub yield_time_ms: u32,
}
fn default_yield_time() -> u32 {
    1000
}

impl RunCommandArgs {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.yield_time_ms <= 60000,
            "yield_time_ms must not exceed 60000"
        );
        ensure!(
            !self.program.trim().is_empty() && !self.program.contains('\0'),
            "invalid command program"
        );
        ensure!(
            self.args.iter().all(|arg| !arg.contains('\0')),
            "command arguments cannot contain NUL"
        );
        ensure!(
            self.timeout_ms.is_none_or(|value| value > 0),
            "command timeout must be positive"
        );
        ensure!(
            self.environment.len() <= 64,
            "too many command environment variables"
        );
        let mut bytes = 0usize;
        for (key, value) in &self.environment {
            let mut chars = key.chars();
            ensure!(
                chars
                    .next()
                    .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
                    && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_'),
                "invalid command environment variable name"
            );
            let upper = key.to_ascii_uppercase();
            ensure!(
                !upper.starts_with("KYLER_")
                    && !upper.starts_with("LD_")
                    && !upper.starts_with("DYLD_")
                    && !matches!(
                        upper.as_str(),
                        "HOME"
                            | "TMPDIR"
                            | "TMP"
                            | "TEMP"
                            | "BASH_ENV"
                            | "ENV"
                            | "ZDOTDIR"
                            | "SHELLOPTS"
                            | "BASHOPTS"
                            | "NODE_OPTIONS"
                    ),
                "command environment variable is reserved: {key}"
            );
            ensure!(
                !value.contains('\0'),
                "command environment values cannot contain NUL"
            );
            bytes = bytes.saturating_add(key.len()).saturating_add(value.len());
            if upper == "PATH" {
                ensure!(
                    !value.is_empty()
                        && std::env::split_paths(value).all(|path| path.is_absolute()),
                    "command PATH must contain absolute directories without empty entries"
                );
            }
        }
        ensure!(bytes <= 32768, "command environment exceeds size limit");
        Ok(())
    }
    pub(crate) fn directory(
        &self,
        base: &Path,
        filesystem: &crate::sandbox::FilesystemPolicy,
    ) -> anyhow::Result<PathBuf> {
        let directory =
            filesystem.check_read(base, self.working_directory.as_deref().unwrap_or(base))?;
        ensure!(
            directory.is_dir(),
            "command working directory must exist and be a directory"
        );
        Ok(directory)
    }
}
pub struct RunCommandTool;
#[async_trait::async_trait]
impl Tool for RunCommandTool {
    fn approval_requirement(&self, args: &Value) -> anyhow::Result<ToolOperation> {
        serde_json::from_value::<RunCommandArgs>(args.clone())?.validate()?;
        Ok(ToolOperation::Command)
    }

    fn activity_kind(&self) -> ActivityKind {
        ActivityKind::Execute
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::ManagedProcess
    }
    fn name(&self) -> &str {
        "run_command"
    }
    fn description(&self) -> &str {
        "Run a program with an argument array inside the configured sandbox. Optionally set working_directory, per-call environment variables, and timeout_ms within the host limit. Use this to inspect projects, prepare local runtimes and dependencies, and execute in the selected environment. Environment variables and directory changes do not persist across calls; reuse explicit executable paths or pass the same settings again. Reserved host and loader variables cannot be overridden. Waits up to yield_time_ms (default 1000, maximum 60000). A completed command returns status, exit_code, stdout and stderr. A running command returns process_id, cursor and current output; use read_process, write_process or stop_process to manage it. yield_time_ms only controls how soon this call returns; timeout_ms limits total execution within the host background timeout. No terminal/PTY is provided. Shell syntax is interpreted only if an explicit shell program is requested. Use direct program and argument arrays for routine workspace operations. Sandbox permissions and output limits cannot be expanded."
    }
    fn parameters(&self) -> Value {
        serde_json::to_value(schema_for!(RunCommandArgs)).expect("command schema is serializable")
    }
    async fn execute(&self, args: &str, context: &mut ToolContext<'_>) -> anyhow::Result<String> {
        let command: RunCommandArgs = serde_json::from_str(args)?;
        command.validate()?;
        let environment = context
            .environment
            .context("run_command requires a configured sandbox")?;
        let manager = context
            .processes
            .context("run_command requires a process manager")?;
        let result = manager
            .run_command(context.session_id, environment.clone(), command)
            .await?;
        Ok(serde_json::to_string(&result)?)
    }
    fn compress_output(
        &self,
        output: &str,
        budget: &ToolOutputBudget,
    ) -> anyhow::Result<CompressedToolOutput> {
        if output.len() <= budget.max_bytes {
            return Ok(CompressedToolOutput::unchanged(output));
        }
        let original: Value = serde_json::from_str(output)?;
        let render = |cap| -> anyhow::Result<String> {
            let mut value = original.clone();
            for field in ["stdout", "stderr"] {
                if let Some(text) = value[field]
                    .as_str()
                    .filter(|s| s.len() > cap)
                    .map(str::to_string)
                {
                    value[field] =
                        Value::String(format!("{}[... omitted]", text_prefix(&text, cap)));
                    value[format!("{field}_truncated")] = Value::Bool(true);
                }
            }
            Ok(serde_json::to_string(&value)?)
        };
        let mut content = render(0)?;
        ensure!(
            content.len() <= budget.max_bytes,
            "command metadata exceeds output budget"
        );
        let (mut low, mut high) = (0, output.len());
        while low < high {
            let mid = low + (high - low).div_ceil(2);
            let candidate = render(mid)?;
            if candidate.len() <= budget.max_bytes {
                content = candidate;
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        CompressedToolOutput::compressed(output, content, budget)
    }
}

/// Session-scoped controls for commands started by run_command.
pub struct ProcessTool {
    pub manager: Arc<ProcessManager>,
    pub action: &'static str,
}
#[async_trait::async_trait]
impl Tool for ProcessTool {
    fn name(&self) -> &str {
        self.action
    }
    fn description(&self) -> &str {
        match self.action {
            "read_process" => {
                "Read status and incremental process output using a cursor. Processes belong to the current session."
            }
            "write_process" => {
                "Write bounded text to a managed process stdin. Set close=true to send EOF."
            }
            _ => "Stop a managed background process and terminate its sandbox process group.",
        }
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"process_id":{"type":"string"},"cursor":{"type":"integer","minimum":0},"data":{"type":"string"},"close":{"type":"boolean"}},"required":["process_id"],"additionalProperties":false})
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Host
    }
    fn activity_kind(&self) -> ActivityKind {
        ActivityKind::Execute
    }
    fn approval_requirement(&self, _: &Value) -> Result<ToolOperation> {
        Ok(match self.action {
            "read_process" => ToolOperation::Read,
            "stop_process" => ToolOperation::State,
            _ => ToolOperation::Command,
        })
    }
    async fn execute(&self, args: &str, context: &mut ToolContext<'_>) -> Result<String> {
        let args: Value = serde_json::from_str(args)?;
        let id = args["process_id"].as_str().unwrap_or_default();
        let value = match self.action {
            "read_process" => serde_json::to_value(self.manager.read(
                context.session_id,
                id,
                args["cursor"].as_u64().unwrap_or(0),
            )?)?,
            "write_process" => {
                self.manager.write(
                    context.session_id,
                    id,
                    ProcessInput {
                        data: args["data"].as_str().unwrap_or_default().into(),
                        close: args["close"].as_bool().unwrap_or(false),
                    },
                )?;
                json!({"written":true})
            }
            _ => {
                self.manager.stop(context.session_id, id)?;
                json!({"cancelled":true})
            }
        };
        Ok(serde_json::to_string(&value)?)
    }
}

#[cfg(test)]
mod environment_tests {
    use super::RunCommandArgs;
    use serde_json::json;

    #[test]
    fn command_context_accepts_project_toolchains_but_rejects_reserved_overrides() {
        let valid: RunCommandArgs = serde_json::from_value(json!({
            "program":".venv/bin/python", "working_directory":"backend",
            "environment":{"JAVA_HOME":"/project/jdk", "VIRTUAL_ENV":"/project/.venv", "PATH":"/project/bin:/usr/bin:/bin"},
            "timeout_ms":1000
        })).unwrap();
        valid.validate().unwrap();
        for key in [
            "KYLER_INTERNAL_TOOL_WORKER",
            "KYLER_LLM_API_KEY",
            "HOME",
            "TMPDIR",
            "DYLD_INSERT_LIBRARIES",
            "LD_PRELOAD",
            "BASH_ENV",
            "NODE_OPTIONS",
        ] {
            let invalid: RunCommandArgs =
                serde_json::from_value(json!({"program":"echo", "environment":{key:"value"}}))
                    .unwrap();
            assert!(invalid.validate().is_err(), "{key}");
        }
        for path in ["", "bin:/usr/bin", "/bin:"] {
            let invalid: RunCommandArgs =
                serde_json::from_value(json!({"program":"echo", "environment":{"PATH":path}}))
                    .unwrap();
            assert!(invalid.validate().is_err());
        }
    }
}
