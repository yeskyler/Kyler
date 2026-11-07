use super::{
    Agent,
    events::{AgentStreamEvent, StreamControl},
};
use crate::{
    context::{ContentItem, Role, TokenUsage},
    session::Session,
};

use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result};
use async_openai::{
    Client,
    config::OpenAIConfig,
    types::chat::{
        ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls, ChatCompletionStreamOptions,
        ChatCompletionStreamResponseDelta, CompletionUsage, CreateChatCompletionRequest,
        FinishReason, FunctionCall,
    },
};
use futures::StreamExt;

// Preserve optional provider-returned reasoning without enabling a reasoning mode.
#[derive(serde::Deserialize)]
struct ModelChunk {
    #[serde(default)]
    choices: Vec<ModelChoice>,
    usage: Option<CompletionUsage>,
}
#[derive(serde::Deserialize)]
struct ModelChoice {
    index: u32,
    delta: ModelDelta,
    finish_reason: Option<FinishReason>,
}
#[derive(serde::Deserialize)]
struct ModelDelta {
    #[serde(flatten)]
    standard: ChatCompletionStreamResponseDelta,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
}

impl Agent {
    pub(super) async fn streaming_response(
        &self,
        client: &Client<OpenAIConfig>,
        mut request: CreateChatCompletionRequest,
        session: &mut Session,
        events: &StreamControl,
    ) -> Result<(Option<String>, Option<Vec<ChatCompletionMessageToolCalls>>)> {
        request.stream_options = Some(ChatCompletionStreamOptions {
            include_usage: Some(true),
            include_obfuscation: None,
        });
        // Never retry after emitting deltas: replay would duplicate user-visible output.
        self.observe(super::observation::RuntimeObservation::ModelRequestStarted);
        request.stream = Some(true);
        let mut stream = tokio::time::timeout(
            Duration::from_secs(60),
            client.chat().create_stream_byot::<_, ModelChunk>(request),
        )
        .await
        .context("stream connection timed out")??;
        let mut text = String::new();
        let mut text_event: Option<usize> = None;
        let mut reasoning_event: Option<usize> = None;
        let mut calls: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
        let mut finished = None;
        let mut first_event = true;
        let mut reported = TokenUsage::default();
        loop {
            let chunk = tokio::time::timeout(Duration::from_secs(60), stream.next())
                .await
                .context("model stream idle timeout")?;
            let Some(chunk) = chunk else { break };
            let chunk = chunk?;
            if first_event {
                self.observe(super::observation::RuntimeObservation::FirstModelEvent);
                first_event = false;
            }
            if let Some(usage) = chunk.usage {
                // Chat Completions usage describes this request, not a per-chunk increment.
                // Account only newly reported usage, retaining known usage on cancellation/error.
                session.add_usage(
                    usage.prompt_tokens.saturating_sub(reported.prompt_tokens),
                    usage
                        .completion_tokens
                        .saturating_sub(reported.completion_tokens),
                    usage.total_tokens.saturating_sub(reported.total_tokens),
                );
                reported.prompt_tokens = reported.prompt_tokens.max(usage.prompt_tokens);
                reported.completion_tokens =
                    reported.completion_tokens.max(usage.completion_tokens);
                reported.total_tokens = reported.total_tokens.max(usage.total_tokens);
                session.memory.record_prompt_tokens(reported.prompt_tokens);
            }
            for choice in chunk.choices.into_iter().filter(|c| c.index == 0) {
                anyhow::ensure!(finished.is_none(), "model sent content after finish_reason");
                if let Some(delta) = choice
                    .delta
                    .reasoning_content
                    .or(choice.delta.reasoning)
                    .filter(|text| !text.is_empty())
                {
                    if let Some(index) = reasoning_event {
                        if let ContentItem::Reasoning { text } =
                            &mut session.events[index].content[0]
                        {
                            text.push_str(&delta);
                        }
                        session.touch();
                    } else {
                        self.record(
                            session,
                            vec![ContentItem::Reasoning {
                                text: delta.clone(),
                            }],
                        )?;
                        reasoning_event = Some(session.events.len() - 1);
                    }
                    text_event = None;
                    events
                        .emit(AgentStreamEvent::ReasoningDelta {
                            step: session.current_execution()?.current_step,
                            text: delta,
                        })
                        .await?;
                }
                if let Some(delta) = choice.delta.standard.content {
                    text.push_str(&delta);
                    if !delta.is_empty() {
                        reasoning_event = None;
                        if let Some(index) = text_event {
                            if let ContentItem::Message { content, .. } =
                                &mut session.events[index].content[0]
                            {
                                content.push_str(&delta);
                            }
                            session.touch();
                        } else {
                            self.record(
                                session,
                                vec![ContentItem::Message {
                                    role: Role::Assistant,
                                    content: delta.clone(),
                                }],
                            )?;
                            text_event = Some(session.events.len() - 1);
                        }
                        events
                            .emit(AgentStreamEvent::TextDelta {
                                step: session.current_execution()?.current_step,
                                text: delta,
                            })
                            .await?;
                    }
                }
                for delta in choice.delta.standard.tool_calls.unwrap_or_default() {
                    let call = calls.entry(delta.index).or_default();
                    if let Some(id) = delta.id {
                        call.0.push_str(&id);
                    }
                    if let Some(function) = delta.function {
                        if let Some(name) = function.name {
                            call.1.push_str(&name);
                        }
                        if let Some(arguments) = function.arguments {
                            call.2.push_str(&arguments);
                        }
                    }
                }
                if let Some(reason) = choice.finish_reason {
                    finished = Some(reason);
                }
            }
        }
        // Calibrate once, against the final known input count, rather than each snapshot.
        session.memory.observe_prompt_tokens(reported.prompt_tokens);
        let reason = finished.context("model stream ended without finish_reason")?;
        anyhow::ensure!(
            matches!(reason, FinishReason::Stop | FinishReason::ToolCalls),
            "model stream did not complete successfully: {reason:?}"
        );
        let calls = calls
            .into_values()
            .map(|(id, name, arguments)| {
                anyhow::ensure!(
                    !id.is_empty() && !name.is_empty(),
                    "incomplete streamed tool call"
                );
                let value: serde_json::Value =
                    serde_json::from_str(&arguments).context("invalid streamed tool arguments")?;
                anyhow::ensure!(value.is_object(), "tool arguments must be an object");
                Ok(ChatCompletionMessageToolCalls::Function(
                    ChatCompletionMessageToolCall {
                        id,
                        function: FunctionCall { name, arguments },
                    },
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((
            (!text.is_empty()).then_some(text),
            (!calls.is_empty()).then_some(calls),
        ))
    }
}
