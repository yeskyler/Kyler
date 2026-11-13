//! Model/tool iteration and step budget enforcement.
use super::{
    Agent,
    events::{AgentStreamEvent, StreamControl},
};
use crate::{
    context::{ContentItem, Role},
    models::error::ModelError,
    session::Session,
    subagent::SubagentScope,
    tools::{ToolExecutionMode, apply_patch::PatchGenerationLimit, is_subagent_management},
};

use std::time::Duration;

use anyhow::{Context, Result};
use async_openai::{
    Client,
    config::OpenAIConfig,
    types::chat::{
        ChatCompletionRequestSystemMessageArgs, CreateChatCompletionRequestArgs,
        CreateChatCompletionResponse,
    },
};
use backon::{ExponentialBuilder, Retryable};

impl Agent {
    pub(super) async fn run_loop(
        &self,
        session: &mut Session,
        scope: Option<&SubagentScope>,
        stream: Option<&StreamControl>,
    ) -> Result<String> {
        let client = Client::with_config(
            OpenAIConfig::new()
                .with_api_base(&self.base_url)
                .with_api_key(&self.api_key),
        );
        let mut explicit_loaded = false;
        let mut patch_retries = 0;
        let mut patch_recovery: Option<String> = None;
        loop {
            let preparing = crate::diagnostics::enabled().then(std::time::Instant::now);
            let step_agent = self.tool_snapshot(&session.session_id).await?;
            if !explicit_loaded {
                step_agent
                    .load_requested_skills(session, scope, stream)
                    .await?;
                explicit_loaded = true;
            }
            let mut tools = step_agent
                .toolbox
                .values()
                .filter(|tool| scope.is_some() || !is_subagent_management(tool.as_ref()))
                .filter(|tool| {
                    step_agent.environment.is_none()
                        || tool.execution_mode() != ToolExecutionMode::Unspecified
                })
                .filter(|tool| {
                    step_agent.environment.as_ref().is_none_or(|environment| {
                        !matches!(
                            tool.name(),
                            "shell" | "exec_command" | "run_command" | "write_process"
                        ) || environment.policy().process.allow_commands
                    })
                })
                .map(|tool| {
                    let mut definition = tool.definition()?;
                    crate::approval::policy::advertise(&mut definition.parameters)?;
                    crate::models::project_tool(definition)
                })
                .collect::<Result<Vec<_>>>()?;
            if self
                .model_info()
                .is_some_and(|model| model.capabilities.tools == Some(false))
            {
                tools.clear();
            }
            if self.observer.is_some() {
                use sha2::{Digest, Sha256};
                // Sort before hashing: toolbox HashMap iteration order is not stable.
                let mut definitions = tools
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                definitions.sort_by_key(|value| {
                    value["function"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned()
                });
                let names = definitions
                    .iter()
                    .filter_map(|value| value["function"]["name"].as_str().map(str::to_owned))
                    .collect();
                let fingerprint =
                    format!("{:x}", Sha256::digest(serde_json::to_vec(&definitions)?));
                self.observe(super::observation::RuntimeObservation::ToolSnapshot {
                    fingerprint,
                    names,
                });
            }
            let execution = session.current_execution_mut()?;
            anyhow::ensure!(
                execution.current_step < self.max_steps,
                "Agent exceeded the maximum of {} steps without a final answer",
                self.max_steps
            );
            execution.current_step += 1;
            tracing::debug!(step = execution.current_step, "agent step started");
            if let Some(stream) = stream {
                stream
                    .emit(AgentStreamEvent::StepStarted {
                        step: execution.current_step,
                    })
                    .await?;
            }
            let mut messages = step_agent
                .prepare_messages(session, &tools, &client, stream)
                .await?;
            if let Some(started) = preparing {
                tracing::info!(target: "kyler_latency", session_id=%session.session_id,
                    execution_id=%session.current_execution()?.execution_id,
                    step=session.current_execution()?.current_step, elapsed_ms=started.elapsed().as_secs_f64()*1000.0, "step preparation latency");
            }
            if let Some(instruction) = &patch_recovery {
                messages.push(
                    ChatCompletionRequestSystemMessageArgs::default()
                        .content(instruction.clone())
                        .build()?
                        .into(),
                );
            }
            let mut builder = CreateChatCompletionRequestArgs::default();
            builder.model(&self.model).messages(messages);
            if !tools.is_empty() {
                builder.tools(tools.clone());
            }
            let mut request = builder.build()?;
            crate::models::set_output_limit(
                &mut request,
                self.output_token_parameter(),
                self.context_policy.output_tokens,
            );
            let (content, tool_calls) = if let Some(stream) = stream {
                match self
                    .streaming_response(&client, request, session, stream)
                    .await
                {
                    Ok(response) => {
                        patch_recovery = None;
                        response
                    }
                    Err(error)
                        if error.downcast_ref::<PatchGenerationLimit>().is_some()
                            && patch_retries < 2 =>
                    {
                        patch_retries += 1;
                        tracing::warn!(
                            retry = patch_retries,
                            "apply_patch generation budget exceeded; requesting a smaller complete call"
                        );
                        // No calls from the rejected batch have executed. Never replay side effects.
                        patch_recovery = Some(format!(
                            "Host feedback: the previous model response was interrupted before any tool in that response executed. {} Return a smaller complete tool call for a coherent subset, substantially reducing the rejected scope; reduce it further if interrupted again. Do not regenerate or resend the rejected full-file patch. If the target file exists, read its relevant section and use Update File; otherwise create a complete smaller functional part and add the remaining parts in later patches. Do not minify code, bypass the budget with shell/exec_command, or repeat already completed work. After all parts are implemented, validate the full result and register the final artifacts.",
                            error
                        ));
                        continue;
                    }
                    Err(error) => return Err(ModelError::wrap(error).into()),
                }
            } else {
                let request = self.provider_request(request, session)?;
                let response = tokio::time::timeout(
                    Duration::from_secs(60),
                    (|| async {
                        self.observe(super::observation::RuntimeObservation::ModelRequestStarted);
                        client
                            .chat()
                            .create_byot::<_, serde_json::Value>(request.clone())
                            .await
                    })
                    .retry(ExponentialBuilder::default().with_max_times(3)),
                )
                .await
                .map_err(|error| ModelError::wrap(error.into()))?
                .map_err(|error| ModelError::wrap(error.into()))?;
                self.observe(super::observation::RuntimeObservation::FirstModelEvent);
                if let Some(text) = response["choices"][0]["message"]["reasoning_content"]
                    .as_str()
                    .or(response["choices"][0]["message"]["reasoning"].as_str())
                    .filter(|text| !text.is_empty())
                {
                    self.record(session, vec![ContentItem::Reasoning { text: text.into() }])?;
                }
                let response: CreateChatCompletionResponse = serde_json::from_value(response)
                    .map_err(|error| ModelError::wrap(error.into()))?;
                if let Some(u) = response.usage {
                    session.memory.observe_prompt_tokens(u.prompt_tokens);
                    session.add_usage(u.prompt_tokens, u.completion_tokens, u.total_tokens);
                }
                let message = response
                    .choices
                    .into_iter()
                    .next()
                    .context("No choices in response")?
                    .message;
                (message.content, message.tool_calls)
            };
            if let Some(calls) = tool_calls.filter(|calls| !calls.is_empty()) {
                if stream.is_none() {
                    if let Some(content) = content.filter(|text| !text.is_empty()) {
                        self.record(
                            session,
                            vec![ContentItem::Message {
                                role: Role::Assistant,
                                content,
                            }],
                        )?;
                    }
                }
                step_agent.record_tool_calls(session, &calls)?;
                // Persist intent before side effects; interrupted calls are never auto-replayed.
                self.checkpoint(session).await?;
                step_agent
                    .execute_tool_calls(session, &calls, scope, stream)
                    .await?;
            } else {
                let output = content.context("No content in final response")?;
                if stream.is_none() {
                    self.record(
                        session,
                        vec![ContentItem::Message {
                            role: Role::Assistant,
                            content: output.clone(),
                        }],
                    )?;
                }
                return Ok(output);
            }
        }
    }
}
