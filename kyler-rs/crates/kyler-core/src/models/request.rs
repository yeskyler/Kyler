//! OpenAI-compatible request parameter projection.
use async_openai::types::chat::{
    ChatCompletionTool, ChatCompletionTools, CreateChatCompletionRequest, FunctionObjectArgs,
};

use kyler_config::OutputTokenParameter;
use kyler_protocol::tool::ToolDefinition;

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
