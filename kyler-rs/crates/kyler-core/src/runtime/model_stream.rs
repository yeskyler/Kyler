use super::{
    Agent,
    events::{AgentStreamEvent, StreamControl},
};
use crate::{
    context::{ContentItem, Role, TokenUsage},
    session::Session,
    tools::apply_patch::{PATCH_GENERATION_TIMEOUT, PatchGenerationLimit, check_argument_size},
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

// Preserve provider-returned reasoning separately from answer text.
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

/// A continuous trickle cannot extend the fixed patch-generation deadline.
async fn read_model_chunk<S: futures::Stream + Unpin>(
    stream: &mut S,
    patch_deadline: Option<tokio::time::Instant>,
) -> Result<Option<S::Item>> {
    if patch_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
        return Err(PatchGenerationLimit("parameter generation exceeded 120 seconds").into());
    }
    let idle_deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let deadline = patch_deadline.map_or(idle_deadline, |deadline| deadline.min(idle_deadline));
    match tokio::time::timeout_at(deadline, stream.next()).await {
        Ok(chunk) => Ok(chunk),
        Err(error) => {
            if patch_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                return Err(
                    PatchGenerationLimit("parameter generation exceeded 120 seconds").into(),
                );
            }
            Err(anyhow::Error::new(error).context("model stream idle timeout"))
        }
    }
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
        let mut timing = crate::diagnostics::ModelTiming::new(session);
        // Never retry after emitting deltas: replay would duplicate user-visible output.
        self.observe(super::observation::RuntimeObservation::ModelRequestStarted);
        request.stream = Some(true);
        let request = self.provider_request(request, session)?;
        if timing.is_some() {
            use sha2::{Digest, Sha256};
            let encoded = serde_json::to_vec(&request)?;
            tracing::info!(target: "kyler_latency", session_id=%session.session_id,
                request_bytes=encoded.len(), request_sha256=%format!("{:x}", Sha256::digest(&encoded)),
                enable_thinking=?request.get("enable_thinking").and_then(|v| v.as_bool()), "model request fingerprint");
        }
        let mut stream = tokio::time::timeout(
            Duration::from_secs(60),
            client.chat().create_stream_byot::<_, ModelChunk>(request),
        )
        .await
        .context("stream connection timed out")??;
        if let Some(timing) = &timing {
            timing.stage("response_headers");
        }
        let mut text = String::new();
        let mut text_event: Option<usize> = None;
        let mut reasoning_event: Option<usize> = None;
        let mut calls: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
        let mut finished = None;
        let mut patch_deadline = None;
        let mut first_event = true;
        let mut reported = TokenUsage::default();
        loop {
            let read_started = timing.as_ref().map(|_| std::time::Instant::now());
            let chunk =
                read_model_chunk(&mut stream, patch_deadline.filter(|_| finished.is_none()))
                    .await?;
            if let (Some(timing), Some(started)) = (&mut timing, read_started) {
                let elapsed = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
                timing.read_wait_us += elapsed;
                timing.max_read_wait_us = timing.max_read_wait_us.max(elapsed);
            }
            let Some(chunk) = chunk else { break };
            let chunk = chunk?;
            if let Some(timing) = &mut timing {
                timing.chunks += 1;
                if first_event {
                    timing.stage("first_model_event");
                }
            }
            if first_event {
                self.observe(super::observation::RuntimeObservation::FirstModelEvent);
                first_event = false;
            }
            if let Some(usage) = chunk.usage {
                if let Some(timing) = &mut timing {
                    timing.usage = Some(serde_json::to_value(&usage)?);
                }
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
                    if let Some(timing) = &mut timing {
                        if timing.reasoning_bytes == 0 {
                            timing.stage("first_reasoning");
                        }
                        timing.reasoning_bytes += delta.len();
                    }
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
                        if let Some(timing) = &mut timing {
                            if timing.text_bytes == 0 {
                                timing.stage("first_text");
                            }
                            timing.text_bytes += delta.len();
                        }
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
                    let previous = (call.0.clone(), call.1.clone());
                    if let Some(id) = delta.id {
                        call.0.push_str(&id);
                    }
                    if let Some(function) = delta.function {
                        if let Some(name) = function.name {
                            call.1.push_str(&name);
                        }
                        if let Some(arguments) = function.arguments {
                            if let Some(timing) = &mut timing {
                                timing.tool_argument_bytes += arguments.len();
                            }
                            call.2.push_str(&arguments);
                        }
                    }
                    if call.1 == "apply_patch" {
                        patch_deadline.get_or_insert_with(|| {
                            tokio::time::Instant::now() + PATCH_GENERATION_TIMEOUT
                        });
                        check_argument_size(call.2.len())?;
                        if finished.is_none()
                            && tokio::time::Instant::now() >= patch_deadline.unwrap()
                        {
                            return Err(PatchGenerationLimit(
                                "parameter generation exceeded 120 seconds",
                            )
                            .into());
                        }
                    }
                    // Only publish identity changes; partial arguments stay private and unparsed.
                    if !call.0.is_empty() && (call.0 != previous.0 || call.1 != previous.1) {
                        events
                            .emit(AgentStreamEvent::ToolCallDelta {
                                call_id: call.0.clone(),
                                name: call.1.clone(),
                            })
                            .await?;
                    }
                }
                if let Some(reason) = choice.finish_reason {
                    finished = Some(reason);
                }
            }
        }
        if let Some(timing) = &timing {
            timing.stage("stream_ended");
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
        if let Some(timing) = &mut timing {
            timing.complete = true;
        }
        Ok((
            (!text.is_empty()).then_some(text),
            (!calls.is_empty()).then_some(calls),
        ))
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[tokio::test]
    async fn continuous_fragments_do_not_reset_patch_deadline() {
        let mut source = Box::pin(futures::stream::unfold((), |_| async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            Some(((), ()))
        }));
        let deadline = tokio::time::Instant::now() + Duration::from_millis(40);
        let error = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match read_model_chunk(&mut source, Some(deadline)).await {
                    Ok(Some(())) => {}
                    Ok(None) => panic!("source should remain open"),
                    Err(error) => break error,
                }
            }
        })
        .await
        .unwrap();
        assert!(error.downcast_ref::<PatchGenerationLimit>().is_some());
    }
}
