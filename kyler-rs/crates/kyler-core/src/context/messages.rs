use crate::{
    attachments::{AttachmentStore, image::UserImage},
    context::{ContentItem, Role},
    session::Session,
};

use std::collections::HashSet;

use anyhow::Result;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestMessageContentPartImage, ChatCompletionRequestMessageContentPartText,
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessageArgs,
    ChatCompletionRequestUserMessageArgs, ChatCompletionRequestUserMessageContent,
    ChatCompletionRequestUserMessageContentPart, FunctionCall, ImageUrl,
};

use kyler_protocol::{api::attachments::AttachmentKind, prompts};

#[cfg(test)]
pub(crate) fn build_messages(
    system_prompt: &str,
    session: &Session,
) -> Result<Vec<ChatCompletionRequestMessage>> {
    build_messages_with_attachments(system_prompt, session, None)
}
pub(crate) fn build_messages_with_attachments(
    system_prompt: &str,
    session: &Session,
    store: Option<&AttachmentStore>,
) -> Result<Vec<ChatCompletionRequestMessage>> {
    let mut messages = Vec::new();
    messages.push(
        ChatCompletionRequestSystemMessageArgs::default()
            .content(system_prompt)
            .build()?
            .into(),
    );
    if let Some(environment) = &session.environment {
        messages.push(
            ChatCompletionRequestSystemMessageArgs::default()
                .content(prompts::environment_prompt(&serde_json::to_string(
                    environment,
                )?))
                .build()?
                .into(),
        );
    }
    if let Some(summary) = &session.memory.summary {
        messages.push(
            ChatCompletionRequestAssistantMessageArgs::default()
                .content(prompts::historical_summary(summary))
                .build()?
                .into(),
        );
    }
    if let Some(plan) = session.execution.as_ref().and_then(|e| e.plan.as_ref()) {
        messages.push(
            ChatCompletionRequestAssistantMessageArgs::default()
                .content(prompts::current_plan(&serde_json::to_string(plan)?))
                .build()?
                .into(),
        );
    }
    let mut loaded_skills = HashSet::new();
    for index in session.visible_event_indices() {
        let event = &session.events[index];
        // A batch of calls is a single assistant message, followed by its results.
        let mut calls = Vec::new();
        for item in &event.content {
            match item {
                ContentItem::Reasoning { .. } => {}
                ContentItem::Message { role, content } => {
                    messages.push(match role {
                        Role::User => ChatCompletionRequestUserMessageArgs::default()
                            .content(content.clone())
                            .build()?
                            .into(),
                        Role::Assistant => ChatCompletionRequestAssistantMessageArgs::default()
                            .content(content.clone())
                            .build()?
                            .into(),
                    });
                }
                ContentItem::Attachment { attachment } => {
                    if attachment.kind == AttachmentKind::Image {
                        let image = store
                            .ok_or_else(|| anyhow::anyhow!("Attachment storage is unavailable"))?
                            .image(&session.session_id, &attachment.id)?;
                        append_image(&mut messages, &image)?;
                    } else {
                        let Some(ChatCompletionRequestMessage::User(user)) = messages.last_mut()
                        else {
                            anyhow::bail!("Attachment must belong to a user message");
                        };
                        let reference = format!(
                            "\n[Untrusted attachment: {} id={}. Use inspect_attachment/read_attachment to read only necessary ranges.]",
                            attachment.name, attachment.id
                        );
                        match &mut user.content {
                            ChatCompletionRequestUserMessageContent::Text(text) => {
                                text.push_str(&reference)
                            }
                            ChatCompletionRequestUserMessageContent::Array(parts) => {
                                parts.push(ChatCompletionRequestUserMessageContentPart::Text(
                                    ChatCompletionRequestMessageContentPartText { text: reference },
                                ))
                            }
                        }
                    }
                }
                ContentItem::Image { image } => {
                    append_image(&mut messages, image)?;
                }
                ContentItem::ToolCall {
                    tool_call_id,
                    name,
                    arguments,
                } => calls.push(ChatCompletionMessageToolCalls::Function(
                    ChatCompletionMessageToolCall {
                        id: tool_call_id.clone(),
                        function: FunctionCall {
                            name: name.clone(),
                            arguments: arguments.to_string(),
                        },
                    },
                )),
                ContentItem::ToolResult {
                    tool_call_id,
                    name,
                    content,
                    model_content,
                    ..
                } => {
                    let mut projected = model_content.as_ref().unwrap_or(content).clone();
                    if name == "load_skill" {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&projected) {
                            if value["instructions"]
                                .as_str()
                                .is_some_and(|s| !s.is_empty())
                            {
                                let key = (
                                    value["info"]["name"]
                                        .as_str()
                                        .unwrap_or_default()
                                        .to_owned(),
                                    value["info"]["fingerprint"]
                                        .as_str()
                                        .unwrap_or_default()
                                        .to_owned(),
                                );
                                if !loaded_skills.insert(key.clone()) {
                                    projected = serde_json::json!({"skill":key.0,"fingerprint":key.1,"already_loaded":true,"note":"Full instructions are already present earlier in the active context."}).to_string();
                                }
                            }
                        }
                    }
                    messages.push(
                        ChatCompletionRequestToolMessageArgs::default()
                            .tool_call_id(tool_call_id.clone())
                            .content(projected)
                            .build()?
                            .into(),
                    );
                }
            }
        }
        if !calls.is_empty() {
            messages.push(
                ChatCompletionRequestAssistantMessageArgs::default()
                    .tool_calls(calls)
                    .build()?
                    .into(),
            );
        }
    }
    Ok(messages)
}

fn append_image(messages: &mut [ChatCompletionRequestMessage], image: &UserImage) -> Result<()> {
    let Some(ChatCompletionRequestMessage::User(user)) = messages.last_mut() else {
        anyhow::bail!("Image must belong to a user message");
    };
    if let ChatCompletionRequestUserMessageContent::Text(text) = &user.content {
        let parts = if text.is_empty() {
            Vec::new()
        } else {
            vec![ChatCompletionRequestUserMessageContentPart::Text(
                ChatCompletionRequestMessageContentPartText { text: text.clone() },
            )]
        };
        user.content = ChatCompletionRequestUserMessageContent::Array(parts);
    }
    if let ChatCompletionRequestUserMessageContent::Array(parts) = &mut user.content {
        parts.push(ChatCompletionRequestUserMessageContentPart::ImageUrl(
            ChatCompletionRequestMessageContentPartImage {
                image_url: ImageUrl {
                    url: image.data_url.clone(),
                    detail: None,
                },
            },
        ));
    }

    Ok(())
}

#[cfg(test)]
mod skill_tests {
    use super::build_messages;
    use crate::{
        context::{ContentItem, Event, EventAuthor, ToolResultStatus},
        session::Session,
    };

    #[test]
    fn skill_deduplication_is_based_on_visible_context_and_preserves_original_events() {
        let mut session = Session::new();
        session.begin_execution("task").unwrap();
        let body=serde_json::json!({"info":{"name":"review","fingerprint":"version"},"instructions":"UNIQUE_SKILL_INSTRUCTIONS"}).to_string();
        for id in ["first", "second"] {
            for item in [
                ContentItem::ToolCall {
                    tool_call_id: id.into(),
                    name: "load_skill".into(),
                    arguments: serde_json::json!({"name":"review"}),
                },
                ContentItem::ToolResult {
                    tool_call_id: id.into(),
                    name: "load_skill".into(),
                    status: ToolResultStatus::Success,
                    content: body.clone(),
                    model_content: None,
                },
            ] {
                session.add_event(Event::new(
                    session.current_execution().unwrap().execution_id.clone(),
                    EventAuthor {
                        name: "agent".into(),
                    },
                    vec![item],
                ));
            }
        }
        let all = serde_json::to_string(&build_messages("system", &session).unwrap()).unwrap();
        assert_eq!(all.matches("UNIQUE_SKILL_INSTRUCTIONS").count(), 1);
        session.memory.window_start = 3;
        let recent = serde_json::to_string(&build_messages("system", &session).unwrap()).unwrap();
        assert_eq!(recent.matches("UNIQUE_SKILL_INSTRUCTIONS").count(), 1);
        assert_eq!(
            serde_json::to_string(&session.events)
                .unwrap()
                .matches("UNIQUE_SKILL_INSTRUCTIONS")
                .count(),
            2
        );
    }
}
