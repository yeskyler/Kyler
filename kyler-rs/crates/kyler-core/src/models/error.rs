//! Safe model diagnostics. Provider bodies are discarded after classification.
use std::fmt::{Display, Formatter};

use async_openai::error::OpenAIError;
use tokio::time::error::Elapsed;

use kyler_protocol::api::{ApiErrorBody, ModelFailureDetails, models::WireProtocol};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Provider(kyler_provider::error::FailureKind),
    PatchGenerationLimit,
}

#[derive(Debug)]
pub struct ModelError {
    kind: Kind,
    details: ModelFailureDetails,
}
impl ModelError {
    pub fn wrap(source: anyhow::Error) -> Self {
        if let Some(existing) = source
            .chain()
            .find_map(|error| error.downcast_ref::<Self>())
        {
            return Self {
                kind: existing.kind,
                details: existing.details.clone(),
            };
        }
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
        let mut details = ModelFailureDetails::default();
        // Codes and parameters are also provider-controlled strings. Publish only
        // known codes and request-field roots, never arbitrary identifier-shaped data.
        fn code(value: Option<&str>) -> Option<String> {
            value
                .filter(|code| {
                    matches!(
                        *code,
                        "invalid_request_error"
                            | "unsupported_parameter"
                            | "unsupported_value"
                            | "invalid_parameter"
                            | "model_not_found"
                            | "invalid_api_key"
                            | "authentication_error"
                            | "permission_denied"
                            | "rate_limit_exceeded"
                            | "rate_limit_error"
                            | "insufficient_quota"
                            | "context_length_exceeded"
                            | "server_error"
                            | "service_unavailable"
                            | "invalid_prompt"
                            | "content_policy_violation"
                            | "invalid_value"
                            | "invalid_model"
                            | "request_timeout"
                    )
                })
                .map(str::to_owned)
        }
        fn parameter(value: Option<&str>) -> Option<String> {
            let root = value?.split(['.', '[']).next()?;
            matches!(
                root,
                "model"
                    | "messages"
                    | "tools"
                    | "tool_choice"
                    | "parallel_tool_calls"
                    | "functions"
                    | "function_call"
                    | "temperature"
                    | "top_p"
                    | "max_tokens"
                    | "max_completion_tokens"
                    | "max_output_tokens"
                    | "reasoning"
                    | "reasoning_effort"
                    | "enable_thinking"
                    | "stream"
                    | "stream_options"
                    | "response_format"
                    | "input"
                    | "instructions"
                    | "include"
                    | "store"
                    | "text"
                    | "metadata"
                    | "stop"
                    | "seed"
                    | "n"
                    | "presence_penalty"
                    | "frequency_penalty"
            )
            .then(|| root.to_owned())
        }
        if let Some(OpenAIError::ApiError(response)) =
            source.chain().find_map(|e| e.downcast_ref::<OpenAIError>())
        {
            details.http_status = Some(response.status_code.as_u16());
            details.provider_code = code(response.api_error.code.as_deref());
            details.parameter = parameter(response.api_error.param.as_deref());
        }
        let body = source
            .chain()
            .find_map(|e| match e.downcast_ref::<OpenAIError>()? {
                OpenAIError::JSONDeserialize(_, body) => Some(body.as_str()),
                OpenAIError::StreamError(error) => match error.as_ref() {
                    async_openai::error::StreamError::EventStream(body) => Some(body.as_str()),
                    _ => None,
                },
                _ => None,
            });
        if let Some(body) = body {
            if body.len() <= 65536 {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
                    let error = value
                        .get("error")
                        .filter(|v| v.is_object())
                        .unwrap_or(&value);
                    details.provider_code = code(error["code"].as_str());
                    details.parameter = parameter(error["param"].as_str());
                }
            }
        }
        details.retryable = matches!(
            kind,
            Kind::Provider(
                kyler_provider::error::FailureKind::Timeout
                    | kyler_provider::error::FailureKind::RateLimit
                    | kyler_provider::error::FailureKind::Unavailable
            )
        );
        Self { kind, details }
    }
    pub fn with_model(mut self, model: &str, protocol: WireProtocol) -> Self {
        self.details.model = Some(model.into());
        self.details.protocol = Some(protocol);
        self
    }
    pub fn public_error(&self) -> ApiErrorBody {
        let (code, message) = match self.kind {
            Kind::Provider(kyler_provider::error::FailureKind::ProtocolIncompatible) => (
                "model_protocol_incompatible",
                "The Chat Completions endpoint cannot combine tools and reasoning for this model. Use Responses or set reasoning to Off.",
            ),
            Kind::Provider(kyler_provider::error::FailureKind::UnsupportedParameter) => (
                "model_parameter_unsupported",
                "The provider rejected a request parameter. Check the parameter and protocol in the error details.",
            ),
            Kind::Provider(kyler_provider::error::FailureKind::ModelNotFound) => (
                "model_not_found",
                "The model is unavailable for this provider or account. Select an available model or check model access.",
            ),
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
                "The model provider returned an invalid or incomplete response. Retry or check the provider's protocol compatibility.",
            ),
            Kind::PatchGenerationLimit => (
                "model_patch_generation_limit",
                "Patch generation exceeded the 16 KiB argument or 120-second generation budget. Automatic retries were exhausted. Request smaller, complete file edits.",
            ),
            Kind::Provider(kyler_provider::error::FailureKind::Request) => (
                "model_request_failed",
                "The model provider rejected this request. See the HTTP status, error code and parameter in the details.",
            ),
        };
        let mut details = self.details.clone();
        if code == "model_protocol_incompatible" {
            details.suggestion = Some(
                "Use protocol=responses for this provider, or explicitly select reasoning Off."
                    .into(),
            );
        }
        let message = if code == "model_request_failed" {
            format!(
                "{}{}{}",
                message,
                details
                    .http_status
                    .map(|status| format!(" HTTP {status}."))
                    .unwrap_or_default(),
                details
                    .provider_code
                    .as_ref()
                    .map(|code| format!(" Provider code: {code}."))
                    .unwrap_or_default()
            )
        } else {
            message.into()
        };
        ApiErrorBody {
            code: code.into(),
            message,
            details: Some(details),
        }
    }
}
impl Display for ModelError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.public_error().message)
    }
}
impl std::error::Error for ModelError {}

#[cfg(test)]
mod tests {
    use super::{Kind, ModelError};
    use kyler_provider::error::classify;

    use async_openai::error::OpenAIError;

    #[test]
    fn protocol_errors_keep_actionable_metadata_without_provider_secrets() {
        let failure = ModelError::wrap(OpenAIError::StreamError(Box::new(async_openai::error::StreamError::EventStream(serde_json::json!({"error":{
            "code":"unsupported_parameter", "param":"reasoning_effort",
            "message":"tools and reasoning_effort cannot be combined in chat/completions; use responses PRIVATE_SECRET"
        }}).to_string()))).into()).with_model("fixture-model", kyler_protocol::api::models::WireProtocol::ChatCompletions);
        let public = failure.public_error();
        assert_eq!(public.code, "model_protocol_incompatible");
        let details = public.details.as_ref().unwrap();
        assert_eq!(details.parameter.as_deref(), Some("reasoning_effort"));
        assert_eq!(
            details.provider_code.as_deref(),
            Some("unsupported_parameter")
        );
        assert_eq!(details.model.as_deref(), Some("fixture-model"));
        assert!(!details.retryable);
        assert!(
            !serde_json::to_string(&public)
                .unwrap()
                .contains("PRIVATE_SECRET")
        );
        assert!(details.suggestion.as_ref().unwrap().contains("responses"));
    }
    #[test]
    fn identifier_shaped_provider_values_are_not_a_secret_exfiltration_channel() {
        let private = "PRIVATE_IDENTIFIER_VALUE";
        let raw = serde_json::json!({"error":{"code":private,"param":private,"message":private}})
            .to_string();
        let failure = ModelError::wrap(
            OpenAIError::StreamError(Box::new(async_openai::error::StreamError::EventStream(raw)))
                .into(),
        );
        let public = failure.public_error();
        assert!(!serde_json::to_string(&public).unwrap().contains(private));
        let details = public.details.unwrap();
        assert!(details.provider_code.is_none() && details.parameter.is_none());
    }
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
        assert!(!format!("{:#}", anyhow::Error::new(failure)).contains("PRIVATE_KEY"));
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
                details: Default::default(),
            }
            .public_error();
            assert_eq!(public.code, expected);
            assert!(!public.message.contains("PRIVATE"));
        }
    }
}
