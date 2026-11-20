//! Provider error classification without exposing response bodies to clients.
use async_openai::error::OpenAIError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    ImageUnsupported,
    Authentication,
    RateLimit,
    ContextLimit,
    Timeout,
    Unavailable,
    InvalidResponse,
    Request,
}

pub fn classify_error(sdk: &OpenAIError) -> FailureKind {
    match sdk {
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
                None => FailureKind::InvalidResponse,
            }
        }
        OpenAIError::Reqwest(error) if error.is_timeout() => FailureKind::Timeout,
        OpenAIError::Reqwest(error) => {
            classify("", "", error.status().map(|status| status.as_u16()))
        }
        OpenAIError::StreamError(_) => FailureKind::InvalidResponse,
        _ => FailureKind::Request,
    }
}
pub fn classify(code: &str, message: &str, status: Option<u16>) -> FailureKind {
    let message = message.to_ascii_lowercase();
    let code = code.to_ascii_lowercase();
    if message.contains("not a vlm")
        || message.contains("does not support image")
        || message.contains("image input is not supported")
        || message.contains("image inputs are not supported")
        || message.contains("image_url is only supported by certain models")
    {
        FailureKind::ImageUnsupported
    } else if matches!(status, Some(401 | 403))
        || matches!(code.as_str(), "invalid_api_key" | "authentication_error")
    {
        FailureKind::Authentication
    } else if status == Some(429)
        || matches!(code.as_str(), "rate_limit_exceeded" | "rate_limit_error")
    {
        FailureKind::RateLimit
    } else if code == "context_length_exceeded"
        || message.contains("maximum context length")
        || message.contains("context window exceeded")
    {
        FailureKind::ContextLimit
    } else if matches!(status, Some(408 | 504)) {
        FailureKind::Timeout
    } else if status.is_some_and(|status| status >= 500) {
        FailureKind::Unavailable
    } else {
        FailureKind::Request
    }
}
