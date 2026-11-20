//! Per-execution model transport with Chat Completions and Responses decoding.
use crate::WireProtocol;
use async_openai::{
    Client,
    config::OpenAIConfig,
    error::OpenAIError,
    types::chat::{ChatCompletionStreamResponseDelta, CompletionUsage, FinishReason},
};
use futures::{StreamExt, stream::BoxStream};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

pub type ChatClient = Client<OpenAIConfig>;
pub fn client(base_url: &str, api_key: &str) -> ChatClient {
    let config = OpenAIConfig::new()
        .with_api_base(base_url)
        .with_api_key(api_key);
    // A plaintext endpoint never negotiates TLS. Avoid synchronous native trust
    // store initialization on runtime threads, while retaining explicit TLS proxies.
    let plaintext = reqwest::Url::parse(base_url).is_ok_and(|url| url.scheme() == "http");
    let tls_proxy = ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
        .iter()
        .any(|key| {
            std::env::var(key).ok().is_some_and(|value| {
                reqwest::Url::parse(&value).is_ok_and(|url| url.scheme() == "https")
            })
        });
    if plaintext && !tls_proxy {
        let mut builder = reqwest::Client::builder();
        if reqwest::Url::parse(base_url).is_ok_and(|url| {
            url.host_str()
                .and_then(|host| host.parse::<std::net::IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback())
        }) {
            builder = builder.no_proxy();
        }
        let http = builder
            .redirect(reqwest::redirect::Policy::none())
            .tls_certs_only([])
            .build()
            .expect("HTTP client configuration");
        Client::build(http, config)
    } else {
        Client::with_config(config)
    }
}
/// Owned by one Agent execution; opaque provider state never crosses sessions.
pub struct ModelSession {
    pub(crate) sdk: ChatClient,
    protocol: WireProtocol,
    pub(crate) continuation: Arc<Mutex<crate::responses::Continuation>>,
}
impl ModelSession {
    pub fn new(sdk: ChatClient, protocol: WireProtocol) -> Self {
        Self {
            sdk,
            protocol,
            continuation: Arc::new(Mutex::new(Default::default())),
        }
    }
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
    client: &ModelSession,
    request: Value,
) -> Result<BoxStream<'static, Result<ModelChunk, OpenAIError>>, OpenAIError> {
    if client.protocol == WireProtocol::Responses {
        crate::responses::stream(client, request).await
    } else {
        Ok(client
            .sdk
            .chat()
            .create_stream_byot::<_, ModelChunk>(request)
            .await?
            .boxed())
    }
}
pub async fn complete(client: &ModelSession, request: Value) -> Result<Value, OpenAIError> {
    if client.protocol == WireProtocol::Responses {
        crate::responses::complete(client, request).await
    } else {
        client.sdk.chat().create_byot(request).await
    }
}
