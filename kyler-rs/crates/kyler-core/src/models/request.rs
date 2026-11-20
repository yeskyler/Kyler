//! OpenAI-compatible request parameter projection.
use async_openai::types::chat::{
    ChatCompletionTool, ChatCompletionTools, CreateChatCompletionRequest, FunctionObjectArgs,
};

use kyler_config::OutputTokenParameter;
use kyler_protocol::{api::models::ThinkingLevel, tool::ToolDefinition};

pub(crate) fn set_output_limit(
    request: &mut CreateChatCompletionRequest,
    parameter: OutputTokenParameter,
    tokens: u32,
) {
    #[allow(deprecated)]
    {
        request.max_tokens = (parameter == OutputTokenParameter::MaxTokens).then_some(tokens);
    }
    request.max_completion_tokens =
        (parameter == OutputTokenParameter::MaxCompletionTokens).then_some(tokens);
}

/// Automatically enable only the known SiliconFlow model. Other endpoints opt in explicitly.
pub(crate) fn default_reasoning_effort(
    base_url: &str,
    model: &str,
    explicit: Option<ThinkingLevel>,
) -> Option<ThinkingLevel> {
    explicit.or_else(|| {
        let adapter = kyler_provider::Adapter::resolve(kyler_provider::AdapterKind::Auto, base_url);
        let capabilities = adapter.reasoning(model, None)?;
        (adapter.0 == kyler_provider::AdapterKind::Siliconflow).then_some(capabilities.default)
    })
}
/// Only current-turn tool continuations carry provider reasoning back to the model.
/// Earlier turns and summaries remain reasoning-free.
pub(crate) fn attach_reasoning(messages: &mut serde_json::Value, session: &crate::Session) {
    use crate::context::ContentItem;
    let Some(execution) = session.execution.as_ref() else {
        return;
    };
    let mut reasoning = String::new();
    let mut by_call = std::collections::HashMap::new();
    for event in session
        .events
        .iter()
        .filter(|e| e.execution_id == execution.execution_id)
    {
        let mut called = false;
        for part in &event.content {
            match part {
                ContentItem::Reasoning { text } => reasoning.push_str(text),
                ContentItem::ToolCall { tool_call_id, .. } => {
                    by_call.insert(tool_call_id.as_str(), reasoning.clone());
                    called = true;
                }
                _ => {}
            }
        }
        if called {
            reasoning.clear();
        }
    }
    if let Some(messages) = messages.as_array_mut() {
        for message in messages {
            if message["role"] != "assistant" {
                continue;
            }
            let text = message["tool_calls"].as_array().and_then(|calls| {
                calls
                    .iter()
                    .find_map(|call| call["id"].as_str().and_then(|id| by_call.get(id)))
            });
            if let Some(text) = text {
                message["reasoning_content"] = text.clone().into();
            }
        }
    }
}

/// SDK-specific conversion belongs at the model boundary, not in tool implementations.
pub(crate) fn project_tool(definition: ToolDefinition) -> anyhow::Result<ChatCompletionTools> {
    let function = FunctionObjectArgs::default()
        .name(definition.name)
        .description(definition.description)
        .parameters(definition.parameters)
        .build()?;
    Ok(ChatCompletionTools::Function(ChatCompletionTool {
        function,
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn neutral_tool_contract_projects_to_chat_completions_without_losing_schema() {
        use crate::tools::{Tool, read_file::ReadFileTool};

        let definition = ReadFileTool.definition().unwrap();
        let encoded =
            serde_json::to_value(super::project_tool(definition.clone()).unwrap()).unwrap();
        assert_eq!(encoded["type"], "function");
        assert_eq!(encoded["function"]["name"], definition.name);
        assert_eq!(encoded["function"]["description"], definition.description);
        assert_eq!(encoded["function"]["parameters"], definition.parameters);
    }
}

#[cfg(test)]
mod thinking_tests {
    use super::default_reasoning_effort;
    use kyler_protocol::api::models::ThinkingLevel;
    #[test]
    fn defaults_only_apply_to_the_known_siliconflow_model_and_allow_opt_out() {
        let model = "deepseek-ai/DeepSeek-V4-Flash";
        assert_eq!(
            default_reasoning_effort("https://api.siliconflow.cn/v1", model, None),
            Some(ThinkingLevel::High)
        );
        assert_eq!(
            default_reasoning_effort("https://api.siliconflow.com/v1", model, None),
            Some(ThinkingLevel::High)
        );
        assert_eq!(
            default_reasoning_effort(
                "https://api.siliconflow.cn/v1",
                model,
                Some(ThinkingLevel::Off)
            ),
            Some(ThinkingLevel::Off)
        );
        assert_eq!(
            default_reasoning_effort("https://api.siliconflow.cn.evil.example/v1", model, None),
            None
        );
        assert_eq!(
            default_reasoning_effort("https://api.openai.com/v1", model, None),
            None
        );
        assert_eq!(
            default_reasoning_effort("https://api.siliconflow.cn/v1", "other-model", None),
            None
        );
        assert_eq!(
            default_reasoning_effort("http://localhost:1234/v1", model, Some(ThinkingLevel::High)),
            Some(ThinkingLevel::High)
        );
    }
}
