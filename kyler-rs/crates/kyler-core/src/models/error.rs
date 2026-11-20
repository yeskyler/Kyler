//! Safe public model failures, with the original cause retained for local diagnostics.
use std::fmt::{Display, Formatter};

use async_openai::error::OpenAIError;
use tokio::time::error::Elapsed;

use kyler_protocol::api::ApiErrorBody;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Provider(kyler_provider::error::FailureKind),
    PatchGenerationLimit,
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
                if error.is::<crate::tools::apply_patch::PatchGenerationLimit>() {
                    return Some(Kind::PatchGenerationLimit);
                }
                if error.is::<Elapsed>() {
                    return Some(Kind::Provider(kyler_provider::error::FailureKind::Timeout));
                }
                error
                    .downcast_ref::<OpenAIError>()
                    .map(kyler_provider::error::classify_error)
                    .map(Kind::Provider)
            })
            .unwrap_or(Kind::Provider(
                kyler_provider::error::FailureKind::InvalidResponse,
            ));
        Self { kind, source }
    }
    pub fn public_error(&self) -> ApiErrorBody {
        let (code, message) = match self.kind {
            Kind::Provider(kyler_provider::error::FailureKind::ImageUnsupported) => (
                "model_image_unsupported",
                "The selected model does not support image input. Use /model to select a vision-capable model, then resend.",
            ),
            Kind::Provider(kyler_provider::error::FailureKind::Authentication) => (
                "model_authentication_failed",
                "The model provider rejected authentication. Check the provider API key and access permissions.",
            ),
            Kind::Provider(kyler_provider::error::FailureKind::RateLimit) => (
                "model_rate_limited",
                "The model provider rate limit was reached. Wait briefly, then resend.",
            ),
            Kind::Provider(kyler_provider::error::FailureKind::ContextLimit) => (
                "model_context_exceeded",
                "The request exceeds the model's context limit. Use a model with a larger context window or start a new session.",
            ),
            Kind::Provider(kyler_provider::error::FailureKind::Timeout) => (
                "model_timeout",
                "The model request timed out. Check the connection and retry.",
            ),
            Kind::Provider(kyler_provider::error::FailureKind::Unavailable) => (
                "model_unavailable",
                "The model provider is temporarily unavailable. Retry later or select another model with /model.",
            ),
            Kind::Provider(kyler_provider::error::FailureKind::InvalidResponse) => (
                "model_invalid_response",
                "The model provider returned an invalid or incomplete response. Retry or check the provider's Chat Completions compatibility.",
            ),
            Kind::PatchGenerationLimit => (
                "model_patch_generation_limit",
                "Patch generation exceeded the 16 KiB argument or 120-second generation budget. Automatic retries were exhausted. Request smaller, complete file edits.",
            ),
            Kind::Provider(kyler_provider::error::FailureKind::Request) => (
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

#[cfg(test)]
mod tests {
    use super::{Kind, ModelError};
    use kyler_provider::error::classify;

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
                kind: Kind::Provider(kind),
                source: anyhow::anyhow!("PRIVATE"),
            }
            .public_error();
            assert_eq!(public.code, expected);
            assert!(!public.message.contains("PRIVATE"));
        }
    }
}
