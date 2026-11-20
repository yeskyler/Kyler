//! Shared Chat Completions transport and wire decoding for compatible adapters.
use async_openai::{
    Client,
    config::OpenAIConfig,
    error::OpenAIError,
    types::chat::{ChatCompletionStreamResponseDelta, CompletionUsage, FinishReason},
};
use futures::Stream;
use serde::Deserialize;
use serde_json::Value;

pub type ChatClient = Client<OpenAIConfig>;
pub fn client(base_url: &str, api_key: &str) -> ChatClient {
    Client::with_config(
        OpenAIConfig::new()
            .with_api_base(base_url)
            .with_api_key(api_key),
    )
}
#[derive(Deserialize)]
pub struct ModelChunk {
    #[serde(default)]
    pub choices: Vec<ModelChoice>,
    pub usage: Option<CompletionUsage>,
}
#[derive(Deserialize)]
pub struct ModelChoice {
    pub index: u32,
    pub delta: ModelDelta,
    pub finish_reason: Option<FinishReason>,
}
#[derive(Deserialize)]
pub struct ModelDelta {
    #[serde(flatten)]
    pub standard: ChatCompletionStreamResponseDelta,
    pub reasoning_content: Option<String>,
    pub reasoning: Option<String>,
}
pub async fn stream(
    client: &ChatClient,
    request: Value,
) -> Result<impl Stream<Item = Result<ModelChunk, OpenAIError>> + Unpin + use<>, OpenAIError> {
    client
        .chat()
        .create_stream_byot::<_, ModelChunk>(request)
        .await
}
pub async fn complete(client: &ChatClient, request: Value) -> Result<Value, OpenAIError> {
    client.chat().create_byot(request).await
}
