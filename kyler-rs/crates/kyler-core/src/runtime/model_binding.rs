//! Model binding and declared input capability checks.
use super::Agent;
use crate::{context::ContextPolicy, models::ModelRegistry};

use std::sync::Arc;

use anyhow::Result;

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
