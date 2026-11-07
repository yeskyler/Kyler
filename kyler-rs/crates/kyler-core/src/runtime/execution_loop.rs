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
    tools::{ToolExecutionMode, is_subagent_management},
};

use std::time::Duration;

use anyhow::{Context, Result};
use async_openai::{Client, config::OpenAIConfig, types::chat::CreateChatCompletionRequestArgs};
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
        loop {
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
                        !matches!(tool.name(), "run_command" | "write_process")
                            || environment.policy().process.allow_commands
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
            let messages = step_agent
                .prepare_messages(session, &tools, &client)
                .await?;
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
                self.streaming_response(&client, request, session, stream)
                    .await
                    .map_err(ModelError::wrap)?
            } else {
                let response = tokio::time::timeout(
                    Duration::from_secs(60),
                    (|| async {
                        self.observe(super::observation::RuntimeObservation::ModelRequestStarted);
                        client.chat().create(request.clone()).await
                    })
                    .retry(ExponentialBuilder::default().with_max_times(3)),
                )
                .await
                .map_err(|error| ModelError::wrap(error.into()))?
                .map_err(|error| ModelError::wrap(error.into()))?;
                self.observe(super::observation::RuntimeObservation::FirstModelEvent);
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
