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
    pub fn model_preference_key(&self) -> String {
        use sha2::{Digest, Sha256};
        self.model_snapshot
            .as_ref()
            .map(|snapshot| snapshot.preference_key())
            .unwrap_or_else(|| {
                format!(
                    "{:x}",
                    Sha256::digest(format!(
                        "embedded\0{}\0{}",
                        self.base_url.trim_end_matches('/'),
                        self.model
                    ))
                )
            })
    }
    /// Explicit effort settings opt custom models in; known non-reasoning OpenAI
    /// models must not receive an automatically injected reasoning parameter.
    pub fn thinking_supported(&self) -> bool {
        !self.thinking_levels().is_empty()
    }
    pub fn default_thinking_level(&self) -> kyler_protocol::api::models::ThinkingLevel {
        use kyler_protocol::api::models::ThinkingLevel;
        self.model_snapshot
            .as_ref()
            .and_then(|s| s.info.capabilities.reasoning.as_ref())
            .map_or(ThinkingLevel::Off, |r| r.default)
    }
    pub fn thinking_enabled(&self) -> bool {
        self.reasoning_effort()
            .is_some_and(kyler_protocol::api::models::ThinkingLevel::enabled)
    }
    pub fn thinking_levels(&self) -> Vec<kyler_protocol::api::models::ThinkingLevel> {
        self.model_snapshot
            .as_ref()
            .and_then(|s| s.info.capabilities.reasoning.as_ref())
            .map_or_else(Vec::new, |r| r.levels.clone())
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
            self.thinking_levels().contains(&level),
            "Thinking level is not supported for this model/provider"
        );
        let snapshot = self
            .model_snapshot
            .as_mut()
            .context("a bound model is required for thinking control")?;
        let snapshot = Arc::make_mut(snapshot);
        snapshot.reasoning_effort = Some(level);
        Ok(self)
    }
    /// Apply explicit user intent independently of provider capability discovery.
    pub fn with_manual_thinking_level(
        mut self,
        level: kyler_protocol::api::models::ThinkingLevel,
    ) -> Result<Self> {
        if let Some(snapshot) = self.model_snapshot.as_mut() {
            let snapshot = Arc::make_mut(snapshot);
            snapshot.manual_reasoning = true;
            snapshot.reasoning_effort = Some(level);
        }
        Ok(self)
    }
    pub(super) fn provider_request(
        &self,
        request: async_openai::types::chat::CreateChatCompletionRequest,
        session: &crate::Session,
    ) -> Result<serde_json::Value> {
        let mut value = serde_json::to_value(request)?;
        let adapter = self
            .model_snapshot
            .as_ref()
            .map(|s| s.adapter)
            .unwrap_or_else(|| {
                kyler_provider::Adapter::resolve(kyler_provider::AdapterKind::Auto, &self.base_url)
            });
        let inferred = adapter.reasoning(&self.model, self.reasoning_effort());
        let capabilities = self
            .model_snapshot
            .as_ref()
            .and_then(|s| s.info.capabilities.reasoning.as_ref())
            .or(inferred.as_ref());
        let manual = self
            .model_snapshot
            .as_ref()
            .is_some_and(|s| s.manual_reasoning);
        let requested = self.reasoning_effort();
        // Off uses a declared disable parameter when available; otherwise omit it.
        let effort = if manual
            && requested == Some(kyler_protocol::api::models::ThinkingLevel::Off)
            && !self
                .model_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.info.capabilities.reasoning.as_ref())
                .is_some_and(|caps| {
                    caps.levels
                        .contains(&kyler_protocol::api::models::ThinkingLevel::Off)
                }) {
            None
        } else {
            requested
        };
        let manual_caps = kyler_protocol::api::models::ReasoningCapabilities {
            levels: kyler_protocol::api::models::ThinkingLevel::ALL.to_vec(),
            default: kyler_protocol::api::models::ThinkingLevel::Off,
            source: "manual".into(),
        };
        adapter.project(
            &mut value,
            &self.model,
            self.output_token_parameter(),
            effort,
            if manual {
                Some(&manual_caps)
            } else {
                capabilities
            },
        )?;
        if self.carries_reasoning() && self.thinking_enabled() {
            crate::models::attach_reasoning(&mut value["messages"], session);
        }
        Ok(value)
    }

    pub(super) fn output_token_parameter(&self) -> OutputTokenParameter {
        self.model_snapshot
            .as_ref()
            .map(|m| m.output_token_parameter)
            .unwrap_or(OutputTokenParameter::MaxTokens)
    }
    pub(super) fn reasoning_effort(&self) -> Option<kyler_protocol::api::models::ThinkingLevel> {
        self.model_snapshot
            .as_ref()
            .map(|m| m.reasoning_effort)
            .unwrap_or_else(|| {
                crate::models::default_reasoning_effort(&self.base_url, &self.model, None)
            })
    }
    fn carries_reasoning(&self) -> bool {
        self.model_snapshot.as_ref().is_some_and(|s| {
            s.info
                .capabilities
                .reasoning_content
                .unwrap_or_else(|| s.adapter.carries_reasoning())
        })
    }
    pub(super) fn request_messages(
        &self,
        messages: Vec<async_openai::types::chat::ChatCompletionRequestMessage>,
        session: &crate::Session,
    ) -> Result<serde_json::Value> {
        let mut value = serde_json::to_value(messages)?;
        if self.thinking_enabled() && self.carries_reasoning() {
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
    fn unknown_models_require_explicit_capability_configuration() {
        let mut config = AppConfig::default();
        config.llm.base_url = "http://localhost:1234/v1".into();
        config.llm.model = "unknown".into();
        config.llm.api_key = SecretString::new("fixture");
        let mut agent = Agent::new(
            config.llm.base_url.clone(),
            "fixture".into(),
            "unknown",
            None::<String>,
            Arc::new(crate::tools::ToolBox::new()),
        );
        agent.model_registry = Some(Arc::new(ModelRegistry::new(&config).unwrap()));
        let bound = agent.bind_model(None).unwrap();
        assert!(bound.thinking_levels().is_empty());
        let request = CreateChatCompletionRequestArgs::default()
            .model("unknown")
            .messages(vec![
                ChatCompletionRequestUserMessageArgs::default()
                    .content("hello")
                    .build()
                    .unwrap()
                    .into(),
            ])
            .build()
            .unwrap();
        let manual = bound
            .clone()
            .with_manual_thinking_level(ThinkingLevel::Ultra)
            .unwrap();
        assert!(manual.thinking_levels().is_empty());
        let value = manual
            .provider_request(request.clone(), &crate::Session::new())
            .unwrap();
        assert_eq!(value["reasoning_effort"], "ultra");
        let off = bound
            .clone()
            .with_manual_thinking_level(ThinkingLevel::Off)
            .unwrap();
        assert!(
            off.provider_request(request, &crate::Session::new())
                .unwrap()
                .get("reasoning_effort")
                .is_none()
        );
        assert!(bound.with_thinking_level(ThinkingLevel::High).is_err());
        config.llm.reasoning_effort = Some(ThinkingLevel::High);
        agent.model_registry = Some(Arc::new(ModelRegistry::new(&config).unwrap()));
        let bound = agent.bind_model(None).unwrap();
        assert_eq!(bound.thinking_levels(), vec![ThinkingLevel::High]);
        assert!(
            bound
                .clone()
                .with_thinking_level(ThinkingLevel::Low)
                .is_err()
        );
        assert!(bound.with_thinking_level(ThinkingLevel::High).is_ok());
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
        assert_eq!(
            bound.thinking_levels(),
            vec![ThinkingLevel::Off, ThinkingLevel::High, ThinkingLevel::Max]
        );
        assert!(
            bound
                .clone()
                .with_thinking_level(ThinkingLevel::Low)
                .is_err()
        );
        for level in bound.thinking_levels() {
            let selected = bound.clone().with_thinking_level(level).unwrap();
            let value = selected
                .provider_request(request.clone(), &crate::Session::new())
                .unwrap();
            assert_eq!(value["enable_thinking"], level.enabled());
            if level.enabled() {
                assert_eq!(value["reasoning_effort"], level.reasoning_effort());
            } else {
                assert!(value.get("reasoning_effort").is_none());
            }
        }
        assert!(bound.thinking_enabled());
        assert_eq!(bound.reasoning_effort(), Some(ThinkingLevel::High));
    }
    #[test]
    fn openai_gpt4_omits_effort_and_gpt5_projects_native_effort_only() {
        for model in ["gpt-4.1-mini", "gpt-5.6-sol"] {
            let mut config = AppConfig::default();
            config.llm.base_url = "https://api.openai.com/v1".into();
            config.llm.model = model.into();
            config.llm.api_key = SecretString::new("fixture");
            let mut agent = Agent::new(
                config.llm.base_url.clone(),
                "fixture".into(),
                model,
                None::<String>,
                Arc::new(crate::tools::ToolBox::new()),
            );
            agent.model_registry = Some(Arc::new(ModelRegistry::new(&config).unwrap()));
            let bound = agent.bind_model(None).unwrap();
            let request = CreateChatCompletionRequestArgs::default()
                .model(model)
                .messages(vec![
                    ChatCompletionRequestUserMessageArgs::default()
                        .content("hello")
                        .build()
                        .unwrap()
                        .into(),
                ])
                .build()
                .unwrap();
            if model == "gpt-4.1-mini" {
                assert!(!bound.thinking_supported());
                let value = bound
                    .provider_request(request, &crate::Session::new())
                    .unwrap();
                assert!(value.get("reasoning_effort").is_none());
                assert!(value.get("enable_thinking").is_none());
            } else {
                for (level, effort) in [
                    (ThinkingLevel::Off, "none"),
                    (ThinkingLevel::Low, "low"),
                    (ThinkingLevel::Medium, "medium"),
                    (ThinkingLevel::High, "high"),
                    (ThinkingLevel::Xhigh, "xhigh"),
                    (ThinkingLevel::Max, "max"),
                ] {
                    let selected = bound.clone().with_thinking_level(level).unwrap();
                    let value = selected
                        .provider_request(request.clone(), &crate::Session::new())
                        .unwrap();
                    assert_eq!(value["reasoning_effort"], effort);
                    assert!(value.get("enable_thinking").is_none());
                }
            }
        }
    }
}
