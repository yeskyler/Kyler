//! Serialized incremental reads for the two model-facing process tools.
use super::ProcessManager;
use crate::{
    environment::ExecutionEnvironment,
    sandbox::ProcessInput,
    tools::{exec_command::WriteStdinArgs, shell::RunCommandArgs},
};

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

impl ProcessManager {
    pub async fn exec_command(
        &self,
        owner: &str,
        environment: ExecutionEnvironment,
        command: RunCommandArgs,
    ) -> Result<Value> {
        let result = self
            .run_command_inner(owner, environment, command, true)
            .await?;
        let id = result["process_id"]
            .as_str()
            .context("missing process session")?
            .to_owned();
        self.continue_command(
            owner,
            WriteStdinArgs {
                session_id: id,
                chars: String::new(),
                yield_time_ms: 0,
                close_stdin: false,
                terminate: false,
            },
        )
        .await
    }

    pub async fn continue_command(&self, owner: &str, args: WriteStdinArgs) -> Result<Value> {
        args.validate()?;
        let interaction = {
            let entries = self.inner.entries.lock().unwrap();
            entries
                .get(&args.session_id)
                .filter(|entry| entry.session == owner)
                .context("Process is unavailable in this chat session")?
                .interaction
                .clone()
        };
        let mut cursor = interaction.lock().await;
        let mut events = self.subscribe();
        let mut input_sent = false;
        let mut input_error = None;
        let before = self.read(owner, &args.session_id, *cursor)?;
        if args.terminate {
            self.stop(owner, &args.session_id)?;
        } else if !args.chars.is_empty() || args.close_stdin {
            if before.status != "running" {
                input_error = Some("Process has ended; input was not sent. Inspect exit_code and remaining output.".to_owned());
            } else {
                match self.write(
                    owner,
                    &args.session_id,
                    ProcessInput {
                        data: args.chars,
                        close: args.close_stdin,
                    },
                ) {
                    Ok(()) => input_sent = true,
                    Err(error) => input_error = Some(error.to_string()),
                }
            }
        }
        let _ = tokio::time::timeout(Duration::from_millis(args.yield_time_ms.into()), async {
            loop {
                let info = self.read(owner, &args.session_id, *cursor)?;
                if info.status != "running"
                    || info
                        .output
                        .iter()
                        .any(|chunk| matches!(chunk.stream.as_str(), "stdout" | "stderr"))
                    || input_error.is_some()
                {
                    return Ok::<_, anyhow::Error>(());
                }
                if events.recv().await.is_err() {
                    tokio::task::yield_now().await;
                }
            }
        })
        .await;
        let info = self.read(owner, &args.session_id, *cursor)?;
        let output = |stream: Option<&str>| {
            info.output
                .iter()
                .filter(|chunk| {
                    matches!(chunk.stream.as_str(), "stdout" | "stderr")
                        && stream.is_none_or(|stream| chunk.stream == stream)
                })
                .map(|chunk| chunk.text.as_str())
                .collect::<String>()
        };
        let truncated = *cursor < info.oldest_cursor.saturating_sub(1);
        *cursor = info.cursor;
        Ok(json!({
            "session_id": if info.status == "running" { Some(args.session_id) } else { None },
            "status": info.status, "exit_code": info.metadata.as_ref().and_then(|metadata| metadata.exit_code),
            "output": output(None),
            "output_truncated": truncated, "input_sent": input_sent, "input_error": input_error, "error": info.error,
        }))
    }
}
