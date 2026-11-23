//! Per-execution model transport with Chat Completions and Responses decoding.
use crate::WireProtocol;
use async_openai::{
    Client,
    config::{Config, OpenAIConfig},
    error::OpenAIError,
    types::chat::{ChatCompletionStreamResponseDelta, CompletionUsage, FinishReason},
};
use futures::{StreamExt, stream::BoxStream};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell};

pub type ChatClient = Client<OpenAIConfig>;
pub fn client(base_url: &str, api_key: &str) -> ChatClient {
    let config = OpenAIConfig::new()
        .with_api_base(base_url.trim_end_matches('/'))
        .with_api_key(api_key);
    let azure = reqwest::Url::parse(base_url).ok().is_some_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some_and(|host| {
                host.ends_with(".openai.azure.com") || host.ends_with(".services.ai.azure.com")
            })
    });
    let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
    if azure && let Ok(mut key) = reqwest::header::HeaderValue::from_str(api_key) {
        key.set_sensitive(true);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("api-key", key);
        builder = builder.default_headers(headers);
    }
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
        if reqwest::Url::parse(base_url).is_ok_and(|url| {
            url.host_str()
                .and_then(|host| host.parse::<std::net::IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback())
        }) {
            builder = builder.no_proxy();
        }
        builder = builder.tls_certs_only([]);
    }
    let http = builder.build().expect("Model HTTP client configuration");
    Client::build(http, config)
}
/// Owned by one Agent execution; opaque provider state never crosses sessions.
pub struct ModelSession {
    pub(crate) sdk: ChatClient,
    protocol: WireProtocol,
    native: OnceCell<NativeTransport>,
    pub(crate) continuation: Arc<Mutex<crate::responses::Continuation>>,
}
impl ModelSession {
    pub fn new(sdk: ChatClient, protocol: WireProtocol) -> Self {
        Self {
            sdk,
            protocol,
            native: OnceCell::new(),
            continuation: Arc::new(Mutex::new(Default::default())),
        }
    }
    async fn native(&self) -> Result<Option<&NativeTransport>, OpenAIError> {
        if !matches!(
            self.protocol,
            WireProtocol::AnthropicMessages | WireProtocol::GeminiGenerateContent
        ) {
            return Ok(None);
        }
        self.native
            .get_or_try_init(|| async {
                let config = self.sdk.config();
                let headers = config.headers();
                let key = headers
                    .get(reqwest::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.strip_prefix("Bearer "))
                    .filter(|key| !key.is_empty())
                    .ok_or_else(|| {
                        OpenAIError::InvalidArgument(
                            "Provider credential is missing or invalid".into(),
                        )
                    })?;
                let transport = match self.protocol {
                    WireProtocol::AnthropicMessages => {
                        crate::anthropic::Messages::new(config.api_base(), key)
                            .await
                            .map(NativeTransport::Anthropic)
                    }
                    WireProtocol::GeminiGenerateContent => {
                        crate::gemini::Gemini::new(config.api_base(), key)
                            .await
                            .map(NativeTransport::Gemini)
                    }
                    _ => unreachable!(),
                };
                transport.map_err(|_| {
                    OpenAIError::InvalidArgument(
                        "Could not initialize native provider transport".into(),
                    )
                })
            })
            .await
            .map(Some)
    }
}

enum NativeTransport {
    Anthropic(crate::anthropic::Messages),
    Gemini(crate::gemini::Gemini),
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
    match client.native().await? {
        Some(NativeTransport::Anthropic(provider)) => provider.stream(request).await,
        Some(NativeTransport::Gemini(provider)) => provider.stream(request).await,
        None if client.protocol == WireProtocol::Responses => {
            crate::responses::stream(client, request).await
        }
        None => Ok(client
            .sdk
            .chat()
            .create_stream_byot::<_, ModelChunk>(request)
            .await?
            .boxed()),
    }
}
pub async fn complete(client: &ModelSession, request: Value) -> Result<Value, OpenAIError> {
    match client.native().await? {
        Some(NativeTransport::Anthropic(provider)) => provider.complete(request).await,
        Some(NativeTransport::Gemini(provider)) => provider.complete(request).await,
        None if client.protocol == WireProtocol::Responses => {
            crate::responses::complete(client, request).await
        }
        None => client.sdk.chat().create_byot(request).await,
    }
}

pub(crate) fn native_status(status_code: reqwest::StatusCode) -> OpenAIError {
    OpenAIError::ApiError(async_openai::error::ApiErrorResponse {
        status_code,
        api_error: async_openai::error::ApiError {
            message: format!("Provider returned HTTP {}", status_code.as_u16()),
            r#type: None,
            param: None,
            code: None,
        },
    })
}

pub(crate) async fn native_http(
    base: &str,
    headers: reqwest::header::HeaderMap,
) -> anyhow::Result<reqwest::Client> {
    let plaintext = base.starts_with("http://");
    let loopback = reqwest::Url::parse(base)
        .ok()
        .and_then(|url| {
            url.host_str()
                .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        })
        .is_some_and(|ip| ip.is_loopback());
    Ok(tokio::task::spawn_blocking(move || {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .default_headers(headers);
        if plaintext {
            builder = builder.tls_certs_only([]);
        }
        if loopback {
            builder = builder.no_proxy();
        }
        builder.build()
    })
    .await??)
}

pub(crate) fn native_events(
    mut response: reqwest::Response,
) -> BoxStream<'static, Result<Value, OpenAIError>> {
    fn invalid() -> OpenAIError {
        OpenAIError::StreamError(Box::new(async_openai::error::StreamError::EventStream(
            "Invalid native provider event stream".into(),
        )))
    }
    Box::pin(async_stream::try_stream! {
        let mut buffer = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            buffer.extend_from_slice(&chunk);
            while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
                if end > 2 * 1024 * 1024 { Err(invalid())?; }
                let line: Vec<_> = buffer.drain(..=end).collect();
                let line = std::str::from_utf8(&line).map_err(|_| invalid())?.trim();
                if let Some(data) = line.strip_prefix("data:") {
                    yield serde_json::from_str(data).map_err(|_| invalid())?;
                }
            }
            if buffer.len() > 2 * 1024 * 1024 { Err(invalid())?; }
        }
        if buffer.iter().any(|byte| !byte.is_ascii_whitespace()) { Err(invalid())?; }
    })
}
