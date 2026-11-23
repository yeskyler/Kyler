//! Model-facing command lifecycle. Legacy worker transport stays private to execution.
use crate::{
    approval::ToolOperation,
    processes::ProcessManager,
    sandbox::ProcessInput,
    tools::{
        ActivityKind, CompressedToolOutput, Tool, ToolContext, ToolExecutionMode, ToolOutputBudget,
        fit_entries, shell::RunCommandArgs, text_prefix,
    },
};

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use anyhow::{Context, Result, ensure};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

fn default_wait() -> u32 {
    1000
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecCommandArgs {
    /// Complete command including arguments, executed by /bin/sh -c inside the sandbox.
    pub command: String,
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub timeout_ms: Option<u32>,
    #[serde(default = "default_wait")]
    pub yield_time_ms: u32,
}
impl ExecCommandArgs {
    pub(crate) fn into_command(self) -> Result<RunCommandArgs> {
        ensure!(
            !self.command.trim().is_empty() && !self.command.contains('\0'),
            "command must contain a complete nonempty shell command"
        );
        ensure!(
            self.command.len() <= 32768,
            "command exceeds 32 KiB; use focused file patches instead of embedding large file contents"
        );
        ensure!(
            cfg!(unix),
            "exec_command currently requires a Unix sandbox shell"
        );
        let command = RunCommandArgs {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), self.command],
            working_directory: self.working_directory,
            environment: self.environment,
            timeout_ms: self.timeout_ms,
            yield_time_ms: self.yield_time_ms,
        };
        command.validate()?;
        Ok(command)
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteStdinArgs {
    /// Process session returned by exec_command, not the chat session ID.
    pub session_id: String,
    /// Omit or send an empty string to wait for and read new output only.
    #[serde(default)]
    pub chars: String,
    #[serde(default = "default_wait")]
    pub yield_time_ms: u32,
    /// Send EOF after any chars. Ordinary pipes have no terminal control characters.
    #[serde(default)]
    pub close_stdin: bool,
    /// Terminate the managed process group; cannot be combined with input or EOF.
    #[serde(default)]
    pub terminate: bool,
}
impl WriteStdinArgs {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            !self.session_id.is_empty() && self.session_id.len() <= 128,
            "invalid process session_id"
        );
        ensure!(self.chars.len() <= 8192, "process input exceeds 8 KiB");
        ensure!(
            self.yield_time_ms <= 60000,
            "yield_time_ms must not exceed 60000"
        );
        ensure!(
            !self.terminate || (self.chars.is_empty() && !self.close_stdin),
            "terminate cannot be combined with chars or close_stdin"
        );
        Ok(())
    }
    pub(crate) fn operation(&self) -> ToolOperation {
        if self.terminate {
            ToolOperation::State
        } else if self.chars.is_empty() && !self.close_stdin {
            ToolOperation::Read
        } else {
            ToolOperation::Command
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Start(ExecCommandArgs),
    Poll {
        process_id: String,
        #[serde(default = "default_wait")]
        yield_time_ms: u32,
    },
    Write {
        process_id: String,
        chars: String,
        #[serde(default = "default_wait")]
        yield_time_ms: u32,
    },
    Close {
        process_id: String,
        #[serde(default = "default_wait")]
        yield_time_ms: u32,
    },
    Terminate {
        process_id: String,
        #[serde(default = "default_wait")]
        yield_time_ms: u32,
    },
}

enum CommandOperation {
    Start(RunCommandArgs),
    Continue(WriteStdinArgs),
}

impl Action {
    fn operation(mut args: Value) -> Result<CommandOperation> {
        // Existing command-only callers still go through the same validation.
        if let Some(object) = args.as_object_mut() {
            object.entry("action").or_insert(json!("start"));
        }
        let (session_id, chars, yield_time_ms, close_stdin, terminate) =
            match serde_json::from_value::<Self>(args)? {
                Self::Start(args) => return Ok(CommandOperation::Start(args.into_command()?)),
                Self::Poll {
                    process_id,
                    yield_time_ms,
                } => (process_id, String::new(), yield_time_ms, false, false),
                Self::Write {
                    process_id,
                    chars,
                    yield_time_ms,
                } => {
                    ensure!(
                        !chars.is_empty(),
                        "write requires nonempty chars; use poll to read"
                    );
                    (process_id, chars, yield_time_ms, false, false)
                }
                Self::Close {
                    process_id,
                    yield_time_ms,
                } => (process_id, String::new(), yield_time_ms, true, false),
                Self::Terminate {
                    process_id,
                    yield_time_ms,
                } => (process_id, String::new(), yield_time_ms, false, true),
            };
        let args = WriteStdinArgs {
            session_id,
            chars,
            yield_time_ms,
            close_stdin,
            terminate,
        };
        args.validate()?;
        Ok(CommandOperation::Continue(args))
    }
}

pub struct ExecCommandTool;
#[async_trait::async_trait]
impl Tool for ExecCommandTool {
    fn name(&self) -> &str {
        "exec_command"
    }
    fn description(&self) -> &str {
        "Manage sandboxed commands. start executes a complete shell command with optional working_directory, environment and timeout_ms. Use the returned process_id with poll for incremental output, write with nonempty chars for stdin, close for EOF, or terminate to stop its process group. Waits use yield_time_ms (default 1000, maximum 60000). A process belongs to the current chat; dependent operations belong in separate batches. Processes have pipes, not a PTY. Inspect exit_code before treating execution as successful."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{
            "action":{"type":"string","enum":["start","poll","write","close","terminate"]},
            "command":{"type":"string","description":"Complete shell command; required for start."},
            "working_directory":{"type":"string","description":"Start only."},
            "environment":{"type":"object","additionalProperties":{"type":"string"},"description":"Start only."},
            "timeout_ms":{"type":"integer","minimum":1,"description":"Total runtime limit; start only."},
            "process_id":{"type":"string","description":"Required for poll, write, close and terminate."},
            "chars":{"type":"string","description":"Required and nonempty for write only."},
            "yield_time_ms":{"type":"integer","minimum":0,"maximum":60000,"default":1000}
        },"required":["action"],"additionalProperties":false})
    }
    fn validate_arguments(&self, args: &Value) -> Result<()> {
        Action::operation(args.clone()).map(|_| ())
    }
    fn approval_requirement(&self, args: &Value) -> Result<ToolOperation> {
        Ok(match Action::operation(args.clone())? {
            CommandOperation::Start(_) => ToolOperation::Command,
            CommandOperation::Continue(args) => args.operation(),
        })
    }
    fn compress_output(
        &self,
        output: &str,
        budget: &ToolOutputBudget,
    ) -> Result<CompressedToolOutput> {
        compact_result(output, budget)
    }
    fn activity_kind(&self) -> ActivityKind {
        ActivityKind::Execute
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::ManagedProcess
    }
    fn concurrency(&self) -> crate::tools::ToolConcurrency {
        crate::tools::ToolConcurrency::Parallel
    }
    async fn execute(&self, args: &str, context: &mut ToolContext<'_>) -> Result<String> {
        self.execute_parallel(args, &context.parallel()).await
    }
    async fn execute_parallel(
        &self,
        args: &str,
        context: &crate::tools::ParallelToolContext<'_>,
    ) -> Result<String> {
        let operation = Action::operation(serde_json::from_str(args)?)?;
        let manager = context
            .processes
            .context("exec_command requires a process manager")?;
        let mut result = match operation {
            CommandOperation::Start(command) => {
                let environment = context
                    .environment
                    .context("exec_command requires an authorized sandbox")?;
                manager
                    .exec_command(context.session_id, environment.clone(), command)
                    .await?
            }
            CommandOperation::Continue(args) => {
                if args.operation() == ToolOperation::Command {
                    ensure!(
                        context
                            .environment
                            .is_some_and(|env| env.policy().process.allow_commands),
                        "Sandbox blocks process input"
                    );
                }
                manager.continue_command(context.session_id, args).await?
            }
        };
        if let Some(fields) = result.as_object_mut()
            && let Some(id) = fields.remove("session_id")
        {
            fields.insert("process_id".into(), id);
        }
        Ok(result.to_string())
    }
}

fn compact_result(output: &str, budget: &ToolOutputBudget) -> Result<CompressedToolOutput> {
    if output.len() <= budget.max_bytes {
        return Ok(CompressedToolOutput::unchanged(output));
    }
    let original: Value = serde_json::from_str(output)?;
    let text = original["output"].as_str().unwrap_or_default();
    let content = fit_entries(text.len(), budget, |cap| {
        let mut value = original.clone();
        value["output"] = Value::String(text_prefix(text, cap).to_owned());
        value["output_truncated"] = Value::Bool(true);
        value.to_string()
    })?;
    CompressedToolOutput::compressed(output, content, budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unified_actions_validate_and_classify_permissions() {
        let tool = ExecCommandTool;
        for (arguments, expected) in [
            (
                json!({"action":"start","command":"echo hello"}),
                ToolOperation::Command,
            ),
            (
                json!({"action":"poll","process_id":"p"}),
                ToolOperation::Read,
            ),
            (
                json!({"action":"write","process_id":"p","chars":"hello"}),
                ToolOperation::Command,
            ),
            (
                json!({"action":"close","process_id":"p"}),
                ToolOperation::Command,
            ),
            (
                json!({"action":"terminate","process_id":"p"}),
                ToolOperation::State,
            ),
        ] {
            assert_eq!(tool.approval_requirement(&arguments).unwrap(), expected);
        }
        for arguments in [
            json!({"action":"start","command":"echo hello","process_id":"p"}),
            json!({"action":"poll","process_id":"p","chars":"x"}),
            json!({"action":"write","process_id":"p","chars":""}),
            json!({"action":"terminate"}),
        ] {
            assert!(tool.validate_arguments(&arguments).is_err(), "{arguments}");
        }
    }

    #[test]
    fn command_requires_complete_input_and_rejects_unsupported_pty() {
        let command = serde_json::from_value::<ExecCommandArgs>(json!({"command":"printf hello"}))
            .unwrap()
            .into_command()
            .unwrap();
        assert_eq!(command.program, "/bin/sh");
        assert_eq!(command.args, ["-c", "printf hello"]);
        assert!(serde_json::from_value::<ExecCommandArgs>(json!({"program":"python3"})).is_err());
        assert!(
            serde_json::from_value::<ExecCommandArgs>(json!({"command":"echo x","pty":true}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<ExecCommandArgs>(json!({"command":" "}))
                .unwrap()
                .into_command()
                .is_err()
        );
    }
    #[test]
    fn empty_stdin_is_a_read_and_lifecycle_options_are_unambiguous() {
        let read: WriteStdinArgs = serde_json::from_value(json!({"session_id":"p"})).unwrap();
        read.validate().unwrap();
        assert_eq!(read.operation(), ToolOperation::Read);
        let eof: WriteStdinArgs =
            serde_json::from_value(json!({"session_id":"p","close_stdin":true})).unwrap();
        assert_eq!(eof.operation(), ToolOperation::Command);
        let invalid: WriteStdinArgs =
            serde_json::from_value(json!({"session_id":"p","terminate":true,"chars":"x"})).unwrap();
        assert!(invalid.validate().is_err());
        let result = json!({"output":"界".repeat(2000),"status":"completed","exit_code":7,"session_id":null});
        let compressed =
            compact_result(&result.to_string(), &ToolOutputBudget { max_bytes: 512 }).unwrap();
        let value: Value = serde_json::from_str(&compressed.content).unwrap();
        assert_eq!(value["exit_code"], 7);
        assert_eq!(value["output_truncated"], true);
        assert!(compressed.content.len() <= 512);
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
