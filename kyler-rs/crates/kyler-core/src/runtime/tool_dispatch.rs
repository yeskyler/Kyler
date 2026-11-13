//! Tool execution routing and model-facing output compression.
use super::{
    Agent,
    events::{AgentStreamEvent, StreamControl},
};
use crate::{
    context::ToolResultStatus,
    session::Session,
    subagent::SubagentScope,
    tools::{ToolContext, ToolExecutionMode},
};

use std::time::Instant;

use anyhow::{Context, Result};
use async_openai::types::chat::ChatCompletionMessageToolCalls;

impl Agent {
    pub(super) fn compress_tool_output(
        &self,
        name: &str,
        status: ToolResultStatus,
        output: &str,
    ) -> Option<String> {
        if status != ToolResultStatus::Success || output.len() <= self.tool_output_budget.max_bytes
        {
            return None;
        }
        let tool = self.toolbox.get(name)?;
        match tool.compress_output(output, &self.tool_output_budget) {
            Ok(result)
                if result.compressed
                    && !result.content.trim().is_empty()
                    && result.content.len() <= self.tool_output_budget.max_bytes
                    && result.content.len() < output.len() =>
            {
                tracing::info!(
                    tool = name,
                    before_bytes = output.len(),
                    after_bytes = result.content.len(),
                    "tool output compressed"
                );
                Some(result.content)
            }
            Ok(result) if !result.compressed && result.content == output => None,
            Ok(_) => {
                tracing::warn!(
                    tool = name,
                    "invalid tool compression result; retaining original output"
                );
                None
            }
            Err(_error) => {
                tracing::warn!(
                    tool = name,
                    "tool output compression failed; retaining original output"
                );
                None
            }
        }
    }

    pub(super) async fn execute_tool_calls(
        &self,
        session: &mut Session,
        calls: &[ChatCompletionMessageToolCalls],
        scope: Option<&SubagentScope>,
        stream: Option<&StreamControl>,
    ) -> Result<()> {
        // Announce the entire batch before authorization or execution can block.
        if let Some(stream) = stream {
            for call in calls {
                if let ChatCompletionMessageToolCalls::Function(call) = call {
                    let arguments = serde_json::from_str(&call.function.arguments)
                        .context("invalid tool arguments")?;
                    let tool = self.toolbox.get(&call.function.name);
                    stream
                        .emit(AgentStreamEvent::ToolQueued {
                            call_id: call.id.clone(),
                            name: call.function.name.clone(),
                            activity: tool
                                .as_ref()
                                .map(|tool| tool.activity_kind())
                                .unwrap_or_default(),
                            source: tool
                                .as_ref()
                                .map(|tool| tool.source_for_arguments(&arguments))
                                .unwrap_or_default(),
                            arguments,
                        })
                        .await?;
                }
            }
        }
        for call in calls {
            let ChatCompletionMessageToolCalls::Function(f) = call else {
                continue;
            };
            let arguments =
                serde_json::from_str(&f.function.arguments).context("invalid tool arguments")?;
            let clean_arguments = crate::approval::policy::arguments(&arguments)
                .map(|(value, _)| value)
                .unwrap_or_else(|_| arguments.clone());
            let execution_arguments = serde_json::to_string(&clean_arguments)?;
            let ambiguous = session.current_execution()?.resumed_from.is_some()
                && session
                    .events
                    .iter()
                    .flat_map(|event| &event.content)
                    .any(|item| {
                        if let crate::context::ContentItem::ToolCall {
                            tool_call_id,
                            name,
                            arguments: old,
                        } = item
                        {
                            let same_command = matches!(
                                name.as_str(),
                                "start_process" | "run_command" | "exec_command" | "shell"
                            ) && matches!(
                                f.function.name.as_str(),
                                "start_process" | "run_command" | "exec_command" | "shell"
                            );
                            (name == &f.function.name || same_command)
                                && old == &arguments
                                && session.events.iter().flat_map(|event| &event.content).any(
                                    |item| match item {
                                        crate::context::ContentItem::ToolResult {
                                            tool_call_id: id,
                                            status,
                                            content,
                                            ..
                                        } if id == tool_call_id => {
                                            *status == ToolResultStatus::Success
                                                || content.to_ascii_lowercase().contains("unknown")
                                        }
                                        _ => false,
                                    },
                                )
                        } else {
                            false
                        }
                    });
            let read_only = self
                .toolbox
                .get(&f.function.name)
                .and_then(|tool| tool.approval_requirement(&clean_arguments).ok())
                .is_some_and(|operation| {
                    matches!(
                        operation,
                        crate::approval::ToolOperation::Read
                            | crate::approval::ToolOperation::State
                    )
                });
            let authorization = if ambiguous && !read_only {
                Err(anyhow::anyhow!(
                    "Previous operation completed or has an unknown outcome. Inspect state; repetition requires a new explicit user instruction."
                ))
            } else {
                self.authorize_tool(session, &f.function.name, &f.id, &arguments)
                    .await
            };
            if authorization.is_ok() {
                if let Some(metadata) = session
                    .current_execution_mut()?
                    .tool_presentations
                    .get_mut(&f.id)
                {
                    metadata.started_at = Some(chrono::Utc::now().timestamp_millis());
                }
            }
            if let Some(stream) = stream.filter(|_| authorization.is_ok()) {
                stream
                    .emit(AgentStreamEvent::ToolStarted {
                        call_id: f.id.clone(),
                        name: f.function.name.clone(),
                        source: self
                            .toolbox
                            .get(&f.function.name)
                            .map(|tool| {
                                tool.source_for_arguments(
                                    &serde_json::from_str(&f.function.arguments)
                                        .unwrap_or_default(),
                                )
                            })
                            .unwrap_or_default(),
                        activity: self
                            .toolbox
                            .get(&f.function.name)
                            .map(|tool| tool.activity_kind())
                            .unwrap_or_default(),
                        arguments: serde_json::from_str(&f.function.arguments)
                            .context("invalid tool arguments")?,
                    })
                    .await?;
            }
            let started = Instant::now();
            tracing::info!(tool=%f.function.name,tool_call_id=%f.id,"tool execution started");
            let result = if let Err(error) = &authorization {
                Err(anyhow::anyhow!("{error:#}"))
            } else if let Some(tool) = self.toolbox.get(&f.function.name) {
                let environment = authorization.expect("authorization was checked above");
                let mut context = ToolContext {
                    processes: self.process_manager.as_deref(),
                    execution: session.execution.as_mut().context("missing execution")?,
                    state: &mut session.state,
                    session_id: &session.session_id,
                    tool_call_id: &f.id,
                    subagents: scope,
                    environment: environment.as_deref(),
                };
                match (environment.as_deref(), tool.execution_mode()) {
                    (Some(environment), ToolExecutionMode::Worker) => {
                        environment
                            .run_tool(&f.function.name, &execution_arguments)
                            .await
                    }

                    (Some(_), ToolExecutionMode::Unspecified) => Err(anyhow::anyhow!(
                        "tool {} has no declared sandbox execution strategy",
                        f.function.name
                    )),
                    _ => tool.execute(&execution_arguments, &mut context).await,
                }
            } else {
                Err(anyhow::anyhow!("unknown tool {}", f.function.name))
            };
            let (status, content) = match result {
                Ok(content) => (ToolResultStatus::Success, content),
                Err(error) => {
                    tracing::warn!(tool=%f.function.name,tool_call_id=%f.id,"tool execution failed");
                    (
                        ToolResultStatus::Error,
                        format!("Tool execution error:{error:#}"),
                    )
                }
            };
            tracing::info!(tool=%f.function.name,tool_call_id=%f.id,status=?status,
                elapsed_ms=started.elapsed().as_millis() as u64,output_bytes=content.len(),"tool execution finished");
            self.record_tool_result(session, &f.id, &f.function.name, status, content.clone())?;
            if let Some(scope) = scope {
                session.record_subagent_tasks(&scope.list());
            }
            self.checkpoint(session).await?;
            if let Some(stream) = stream {
                stream
                    .emit(AgentStreamEvent::ToolFinished {
                        call_id: f.id.clone(),
                        name: f.function.name.clone(),
                        status,
                        output: content,
                    })
                    .await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::super::Agent;
    use crate::{
        context::{ContentItem, ExecutionStatus, ToolResultStatus},
        session::Session,
        tools::{Tool, ToolContext, ToolExecutionMode},
    };

    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use anyhow::Result;
    use async_openai::types::chat::{
        ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls, FunctionCall,
    };

    struct Effect(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl Tool for Effect {
        fn name(&self) -> &str {
            "effect"
        }
        fn description(&self) -> &str {
            "fixture"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type":"object"})
        }
        fn execution_mode(&self) -> ToolExecutionMode {
            ToolExecutionMode::Host
        }
        async fn execute(&self, _: &str, _: &mut ToolContext<'_>) -> Result<String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok("done".into())
        }
    }
    fn call(id: &str) -> ChatCompletionMessageToolCalls {
        ChatCompletionMessageToolCalls::Function(ChatCompletionMessageToolCall {
            id: id.into(),
            function: FunctionCall {
                name: "effect".into(),
                arguments: "{}".into(),
            },
        })
    }
    #[tokio::test]
    async fn resume_does_not_replay_completed_or_unknown_side_effects() {
        for completed in [false, true] {
            let count = Arc::new(AtomicUsize::new(0));
            let agent = Agent::new(
                "http://127.0.0.1:1".into(),
                "key".into(),
                "fixture",
                None::<String>,
                Arc::new(
                    [(
                        "effect".into(),
                        Arc::new(Effect(count.clone())) as Arc<dyn Tool>,
                    )]
                    .into(),
                ),
            );
            let mut session = Session::new();
            session.begin_execution("task").unwrap();
            let previous = session.current_execution().unwrap().execution_id.clone();
            agent
                .record_tool_calls(&mut session, &[call("old")])
                .unwrap();
            if completed {
                agent
                    .record_tool_result(
                        &mut session,
                        "old",
                        "effect",
                        ToolResultStatus::Success,
                        "done".into(),
                    )
                    .unwrap();
            }
            session
                .finish(ExecutionStatus::Failed, None, Some("interrupted".into()))
                .unwrap();
            session.begin_execution("continue").unwrap();
            session.current_execution_mut().unwrap().resumed_from = Some(previous);
            agent
                .record_tool_calls(&mut session, &[call("new")])
                .unwrap();
            agent
                .execute_tool_calls(&mut session, &[call("new")], None, None)
                .await
                .unwrap();
            assert_eq!(count.load(Ordering::SeqCst), 0);
            assert!(session.events.iter().flat_map(|event|&event.content).any(|item|matches!(item,ContentItem::ToolResult{tool_call_id,status:ToolResultStatus::Error,content,..} if tool_call_id=="new"&&content.contains("new explicit user instruction"))));
        }
    }
}
