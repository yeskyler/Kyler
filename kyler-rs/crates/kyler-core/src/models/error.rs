//! Safe public model failures, with the original cause retained for local diagnostics.
use std::fmt::{Display, Formatter};

use async_openai::error::OpenAIError;
use tokio::time::error::Elapsed;

use kyler_protocol::api::ApiErrorBody;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    ImageUnsupported,
    Authentication,
    RateLimit,
    ContextLimit,
    Timeout,
    Unavailable,
    InvalidResponse,
    Request,
}

#[derive(Debug)]
pub struct ModelError {
    kind: Kind,
    source: anyhow::Error,
}
impl ModelError {
    pub fn wrap(source: anyhow::Error) -> Self {
        let kind = source
            .chain()
            .find_map(|error| {
                if error.is::<Elapsed>() {
                    return Some(Kind::Timeout);
                }
                let sdk = error.downcast_ref::<OpenAIError>()?;
                Some(match sdk {
                    OpenAIError::ApiError(response) => classify(
                        response.api_error.code.as_deref().unwrap_or_default(),
                        &response.api_error.message,
                        Some(response.status_code.as_u16()),
                    ),
                    OpenAIError::JSONDeserialize(_, body) => {
                        // Compatible providers may return a flat {code,message} envelope.
                        let parsed = (body.len() <= 65536)
                            .then(|| serde_json::from_str::<serde_json::Value>(body).ok())
                            .flatten();
                        match parsed {
                            Some(value) => {
                                let value = value
                                    .get("error")
                                    .filter(|v| v.is_object())
                                    .unwrap_or(&value);
                                let code = value
                                    .get("code")
                                    .map(|v| {
                                        v.as_str()
                                            .map(str::to_owned)
                                            .unwrap_or_else(|| v.to_string())
                                    })
                                    .unwrap_or_default();
                                classify(&code, value["message"].as_str().unwrap_or_default(), None)
                            }
                            None => Kind::InvalidResponse,
                        }
                    }
                    OpenAIError::Reqwest(error) if error.is_timeout() => Kind::Timeout,
                    OpenAIError::Reqwest(error) => {
                        classify("", "", error.status().map(|status| status.as_u16()))
                    }
                    OpenAIError::StreamError(_) => Kind::InvalidResponse,
                    _ => Kind::Request,
                })
            })
            .unwrap_or(Kind::InvalidResponse);
        Self { kind, source }
    }
    pub fn public_error(&self) -> ApiErrorBody {
        let (code, message) = match self.kind {
            Kind::ImageUnsupported => (
                "model_image_unsupported",
                "The selected model does not support image input. Use /model to select a vision-capable model, then resend.",
            ),
            Kind::Authentication => (
                "model_authentication_failed",
                "The model provider rejected authentication. Check the provider API key and access permissions.",
            ),
            Kind::RateLimit => (
                "model_rate_limited",
                "The model provider rate limit was reached. Wait briefly, then resend.",
            ),
            Kind::ContextLimit => (
                "model_context_exceeded",
                "The request exceeds the model's context limit. Use a model with a larger context window or start a new session.",
            ),
            Kind::Timeout => (
                "model_timeout",
                "The model request timed out. Check the connection and retry.",
            ),
            Kind::Unavailable => (
                "model_unavailable",
                "The model provider is temporarily unavailable. Retry later or select another model with /model.",
            ),
            Kind::InvalidResponse => (
                "model_invalid_response",
                "The model provider returned an invalid or incomplete response. Retry or check the provider's Chat Completions compatibility.",
            ),
            Kind::Request => (
                "model_request_failed",
                "The model request failed. Check the selected model and provider configuration.",
            ),
        };
        ApiErrorBody {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl Display for ModelError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.public_error().message)
    }
}
impl std::error::Error for ModelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
fn classify(code: &str, message: &str, status: Option<u16>) -> Kind {
    let message = message.to_ascii_lowercase();
    let code = code.to_ascii_lowercase();
    if message.contains("not a vlm")
        || message.contains("does not support image")
        || message.contains("image input is not supported")
        || message.contains("image inputs are not supported")
        || message.contains("image_url is only supported by certain models")
    {
        Kind::ImageUnsupported
    } else if matches!(status, Some(401 | 403))
        || matches!(code.as_str(), "invalid_api_key" | "authentication_error")
    {
        Kind::Authentication
    } else if status == Some(429)
        || matches!(code.as_str(), "rate_limit_exceeded" | "rate_limit_error")
    {
        Kind::RateLimit
    } else if code == "context_length_exceeded"
        || message.contains("maximum context length")
        || message.contains("context window exceeded")
    {
        Kind::ContextLimit
    } else if matches!(status, Some(408 | 504)) {
        Kind::Timeout
    } else if status.is_some_and(|status| status >= 500) {
        Kind::Unavailable
    } else {
        Kind::Request
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelError, classify};

    use async_openai::error::OpenAIError;

    #[test]
    fn flat_provider_vision_errors_are_classified_without_exposing_raw_details() {
        let parse_error = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let raw = r#"{"code":20041,"message":"The model is not a VLM (Vision Language Model). Please use text-only prompts. PRIVATE_KEY data:image/png;base64,PRIVATE_IMAGE","data":null}"#;
        let failure =
            ModelError::wrap(OpenAIError::JSONDeserialize(parse_error, raw.into()).into());
        let public = failure.public_error();
        assert_eq!(public.code, "model_image_unsupported");
        assert!(public.message.contains("/model"));
        assert!(!format!("{public:?}").contains("PRIVATE"));
        assert!(format!("{:#}", anyhow::Error::new(failure)).contains("PRIVATE_KEY"));
    }
    #[test]
    fn structured_provider_errors_have_safe_actionable_messages() {
        for (status, code, expected) in [
            (401, "invalid_api_key", "model_authentication_failed"),
            (429, "rate_limit_exceeded", "model_rate_limited"),
            (400, "context_length_exceeded", "model_context_exceeded"),
            (503, "", "model_unavailable"),
            (504, "", "model_timeout"),
        ] {
            let kind = classify(code, "PRIVATE_PROVIDER_TEXT", Some(status));
            let public = ModelError {
                kind,
                source: anyhow::anyhow!("PRIVATE"),
            }
            .public_error();
            assert_eq!(public.code, expected);
            assert!(!public.message.contains("PRIVATE"));
        }
    }
}
