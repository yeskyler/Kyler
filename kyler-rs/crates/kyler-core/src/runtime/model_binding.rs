//! Model binding and declared input capability checks.
use super::Agent;
use crate::{context::ContextPolicy, models::ModelRegistry};

use std::sync::Arc;

use anyhow::{Context, Result};

use kyler_config::OutputTokenParameter;
use kyler_protocol::api::models::ModelInfo;

impl Agent {
    pub fn model_registry(&self) -> Option<&Arc<ModelRegistry>> {
        self.model_registry.as_ref()
    }
    /// Bind once at admission; the returned runtime and its children retain this generation.
    pub fn bind_model(&self, profile: Option<&str>) -> Result<Self> {
        let mut agent = self.clone();
        if let Some(registry) = &self.model_registry {
            let snapshot = registry.resolve(profile)?;
            agent.base_url = snapshot.base_url.clone();
            agent.api_key = snapshot.api_key.expose().to_owned();
            agent.model = snapshot.info.model.clone();
            agent.context_policy = snapshot.policy.clone();
            agent.model_snapshot = Some(snapshot);
            agent.model_registry = None;
        }
        Ok(agent)
    }
    /// A bound model can accept an explicit user preference; provider compatibility
    /// is validated by the provider, not guessed from a local model-name allowlist.
    pub fn thinking_supported(&self) -> bool {
        self.model_snapshot.is_some()
    }
    pub fn thinking_enabled(&self) -> bool {
        self.thinking_mode().unwrap_or(false)
    }
    pub fn thinking_levels(&self) -> Vec<kyler_protocol::api::models::ThinkingLevel> {
        use kyler_protocol::api::models::ThinkingLevel::{High, Max};
        if !self.thinking_supported() {
            return Vec::new();
        }
        vec![High, Max]
    }
    pub fn with_thinking(self, enabled: bool) -> Result<Self> {
        self.with_thinking_level(kyler_protocol::api::models::ThinkingLevel::from_enabled(
            enabled,
        ))
    }
    pub fn with_thinking_level(
        mut self,
        level: kyler_protocol::api::models::ThinkingLevel,
    ) -> Result<Self> {
        anyhow::ensure!(
            self.thinking_supported(),
            "Thinking level is not supported for this model/provider"
        );
        let snapshot = self
            .model_snapshot
            .as_mut()
            .context("a bound model is required for thinking control")?;
        let snapshot = Arc::make_mut(snapshot);
        snapshot.enable_thinking = Some(level.enabled());
        snapshot.thinking_level = Some(level);
        Ok(self)
    }
    pub(super) fn provider_request(
        &self,
        request: async_openai::types::chat::CreateChatCompletionRequest,
        session: &crate::Session,
    ) -> Result<serde_json::Value> {
        use kyler_protocol::api::models::ThinkingLevel;
        let mut value = crate::models::provider_request(request, session, self.thinking_mode())?;
        let explicit_level = self
            .model_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.thinking_level);
        let known_default =
            crate::models::thinking_mode(&self.base_url, &self.model, None).is_some();
        if self.thinking_mode() == Some(true) && (explicit_level.is_some() || known_default) {
            value["reasoning_effort"] =
                serde_json::to_value(explicit_level.unwrap_or(ThinkingLevel::High))?;
        } else if self.thinking_mode() == Some(false) {
            value.as_object_mut().unwrap().remove("reasoning_effort");
        }
        Ok(value)
    }
    pub(super) fn output_token_parameter(&self) -> OutputTokenParameter {
        self.model_snapshot
            .as_ref()
            .map(|m| m.output_token_parameter)
            .unwrap_or(OutputTokenParameter::MaxTokens)
    }
    pub(super) fn thinking_mode(&self) -> Option<bool> {
        self.model_snapshot
            .as_ref()
            .map(|m| m.enable_thinking)
            .unwrap_or_else(|| crate::models::thinking_mode(&self.base_url, &self.model, None))
    }
    pub(super) fn request_messages(
        &self,
        messages: Vec<async_openai::types::chat::ChatCompletionRequestMessage>,
        session: &crate::Session,
    ) -> Result<serde_json::Value> {
        let mut value = serde_json::to_value(messages)?;
        if self.thinking_mode() == Some(true) {
            crate::models::attach_reasoning(&mut value, session);
        }
        Ok(value)
    }
    pub fn model_info(&self) -> Option<ModelInfo> {
        self.model_snapshot.as_ref().map(|m| m.info.clone())
    }
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn sliding_window_tokens(&self) -> usize {
        self.context_policy.sliding_window_tokens()
    }
    pub fn context_policy(&self) -> &ContextPolicy {
        &self.context_policy
    }
    pub fn context_window_tokens(&self) -> usize {
        self.context_policy.context_tokens
    }
    /// Adds validated images to this turn without changing the text-only entry points.
    pub fn validate_document_input(&self) -> Result<()> {
        anyhow::ensure!(
            self.model_info()
                .is_none_or(|model| model.capabilities.tools != Some(false)),
            "The selected model cannot call attachment-reading tools. Select a tool-capable model with /model."
        );
        Ok(())
    }
    pub fn validate_image_input(&self) -> Result<()> {
        if self
            .model_info()
            .is_some_and(|model| model.capabilities.images == Some(false))
        {
            anyhow::bail!(
                "The selected model does not support image input. Select a vision-capable model with /model."
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod thinking_level_tests {
    use super::*;
    use async_openai::types::chat::{
        ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
    };
    use kyler_config::{AppConfig, SecretString};
    use kyler_protocol::api::models::ThinkingLevel;

    #[test]
    fn explicit_levels_work_for_unrecognized_model_names_without_changing_defaults() {
        let mut config = AppConfig::default();
        config.llm.base_url = "https://api.siliconflow.cn/v1".into();
        config.llm.model = "deepseek-ai/DeepSeek-V4.1-Flash".into();
        config.llm.api_key = SecretString::new("fixture-only");
        let mut agent = Agent::new(
            config.llm.base_url.clone(),
            "fixture".into(),
            &config.llm.model,
            None::<String>,
            Arc::new(crate::tools::ToolBox::new()),
        );
        agent.model_registry = Some(Arc::new(ModelRegistry::new(&config).unwrap()));
        let bound = agent.bind_model(None).unwrap();
        assert_eq!(
            bound.thinking_levels(),
            vec![ThinkingLevel::High, ThinkingLevel::Max]
        );
        let request = CreateChatCompletionRequestArgs::default()
            .model(&config.llm.model)
            .messages(vec![
                ChatCompletionRequestUserMessageArgs::default()
                    .content("hello")
                    .build()
                    .unwrap()
                    .into(),
            ])
            .build()
            .unwrap();
        let default = bound
            .provider_request(request.clone(), &crate::Session::new())
            .unwrap();
        assert!(default.get("enable_thinking").is_none());
        assert!(default.get("reasoning_effort").is_none());
        for (level, effort) in [
            (ThinkingLevel::Off, None),
            (ThinkingLevel::High, Some("high")),
            (ThinkingLevel::Max, Some("max")),
        ] {
            let value = bound
                .clone()
                .with_thinking_level(level)
                .unwrap()
                .provider_request(request.clone(), &crate::Session::new())
                .unwrap();
            assert_eq!(value["enable_thinking"], level.enabled());
            assert_eq!(
                value
                    .get("reasoning_effort")
                    .and_then(|value| value.as_str()),
                effort
            );
        }
    }

    #[test]
    fn siliconflow_levels_project_distinct_parameters_without_mutating_defaults() {
        let mut config = AppConfig::default();
        config.llm.base_url = "https://api.siliconflow.cn/v1".into();
        config.llm.model = "deepseek-ai/DeepSeek-V4-Flash".into();
        config.llm.api_key = SecretString::new("fixture-only");
        let mut agent = Agent::new(
            config.llm.base_url.clone(),
            "fixture".into(),
            &config.llm.model,
            None::<String>,
            Arc::new(crate::tools::ToolBox::new()),
        );
        agent.model_registry = Some(Arc::new(ModelRegistry::new(&config).unwrap()));
        let bound = agent.bind_model(None).unwrap();
        let request = CreateChatCompletionRequestArgs::default()
            .model(&config.llm.model)
            .messages(vec![
                ChatCompletionRequestUserMessageArgs::default()
                    .content("hello")
                    .build()
                    .unwrap()
                    .into(),
            ])
            .build()
            .unwrap();
        for (level, effort) in [
            (ThinkingLevel::Off, None),
            (ThinkingLevel::High, Some("high")),
            (ThinkingLevel::Max, Some("max")),
        ] {
            let selected = bound.clone().with_thinking_level(level).unwrap();
            let value = selected
                .provider_request(request.clone(), &crate::Session::new())
                .unwrap();
            assert_eq!(value["enable_thinking"], level.enabled());
            if level == ThinkingLevel::Off {
                assert!(value.get("reasoning_effort").is_none());
            }
            assert_eq!(
                value
                    .get("reasoning_effort")
                    .and_then(|value| value.as_str()),
                effort
            );
        }
        assert!(bound.thinking_enabled());
        assert!(
            bound
                .model_snapshot
                .as_ref()
                .unwrap()
                .thinking_level
                .is_none()
        );
    }
}
