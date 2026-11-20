use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use kyler_protocol::api::models::{ModelCapabilities, ReasoningCapabilities, ThinkingLevel};

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    #[default]
    Auto,
    Openai,
    Siliconflow,
    Generic,
}
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputTokenParameter {
    #[default]
    Auto,
    MaxTokens,
    MaxCompletionTokens,
}
pub use kyler_protocol::api::models::WireProtocol;

#[derive(Debug, Clone, Copy)]
pub struct Adapter(pub AdapterKind);
impl Adapter {
    pub fn protocol(self, model: &str, configured: WireProtocol) -> WireProtocol {
        if configured != WireProtocol::Auto {
            return configured;
        }
        if self.0 == AdapterKind::Openai
            && (model.starts_with("gpt-5")
                || model.starts_with("gpt-6")
                || model.starts_with("o1")
                || model.starts_with("o3")
                || model.starts_with("o4"))
        {
            WireProtocol::Responses
        } else {
            WireProtocol::ChatCompletions
        }
    }

    pub fn resolve(kind: AdapterKind, base_url: &str) -> Self {
        if kind != AdapterKind::Auto {
            return Self(kind);
        }
        Self(
            match url::Url::parse(base_url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .as_deref()
            {
                Some("api.openai.com") => AdapterKind::Openai,
                Some("api.siliconflow.cn" | "api.siliconflow.com") => AdapterKind::Siliconflow,
                _ => AdapterKind::Generic,
            },
        )
    }
    pub fn selectable_model(self, id: &str) -> bool {
        if self.0 == AdapterKind::Openai {
            return (id.starts_with("gpt-")
                || id.starts_with("chatgpt-")
                || id.starts_with("o1")
                || id.starts_with("o3")
                || id.starts_with("o4"))
                && ![
                    "audio",
                    "realtime",
                    "transcribe",
                    "tts",
                    "image",
                    "search",
                    "codex",
                    "pro",
                ]
                .iter()
                .any(|part| id.contains(part));
        }
        ![
            "embedding",
            "rerank",
            "bge-",
            "whisper",
            "stable-diffusion",
            "flux",
            "speech",
        ]
        .iter()
        .any(|part| id.to_lowercase().contains(part))
    }
    pub fn output_parameter(
        self,
        model: &str,
        configured: OutputTokenParameter,
    ) -> OutputTokenParameter {
        if configured != OutputTokenParameter::Auto {
            return configured;
        }
        match self.0 {
            AdapterKind::Openai => OutputTokenParameter::MaxCompletionTokens,
            AdapterKind::Generic
                if model.starts_with("gpt-5")
                    || model.starts_with("o1")
                    || model.starts_with("o3")
                    || model.starts_with("o4") =>
            {
                OutputTokenParameter::MaxCompletionTokens
            }
            _ => OutputTokenParameter::MaxTokens,
        }
    }
    pub fn reasoning(
        self,
        model: &str,
        explicit: Option<ThinkingLevel>,
    ) -> Option<ReasoningCapabilities> {
        use ThinkingLevel::*;
        let levels = match self.0 {
            AdapterKind::Openai if model == "gpt-5.6" || model.starts_with("gpt-5.6-sol") => {
                vec![Off, Low, Medium, High, Xhigh, Max]
            }
            AdapterKind::Openai
                if model.starts_with("gpt-4")
                    || model.starts_with("chatgpt-4")
                    || model.starts_with("gpt-3.5") =>
            {
                vec![]
            }
            AdapterKind::Siliconflow
                if matches!(
                    model,
                    "Pro/deepseek-ai/DeepSeek-V4"
                        | "deepseek-ai/DeepSeek-V4-Flash"
                        | "Pro/zai-org/GLM-5.2"
                ) =>
            {
                vec![Off, High, Max]
            }
            _ if explicit.is_some() => vec![explicit.unwrap()],
            _ => return None,
        };
        let default = explicit.filter(|v| levels.contains(v)).unwrap_or(
            if self.0 == AdapterKind::Siliconflow {
                High
            } else {
                Medium
            },
        );
        Some(ReasoningCapabilities {
            default: if levels.is_empty() { Off } else { default },
            levels,
            source: if explicit.is_some() {
                "configuration"
            } else {
                "adapter"
            }
            .into(),
        })
    }
    pub fn capabilities(self, model: &str, explicit: Option<ThinkingLevel>) -> ModelCapabilities {
        let mut caps = ModelCapabilities {
            reasoning: self.reasoning(model, explicit),
            ..Default::default()
        };
        if self.0 == AdapterKind::Openai && (model == "gpt-5.6" || model.starts_with("gpt-5.6-sol"))
        {
            caps.images = Some(true);
            caps.tools = Some(true);
            caps.structured_output = Some(true);
            caps.max_context_tokens = Some(1_050_000);
            caps.max_output_tokens = Some(128_000);
        }
        caps
    }
    pub fn carries_reasoning(self) -> bool {
        self.0 == AdapterKind::Siliconflow
    }
    /// Convert unified intent only after the host has projected its session.
    pub fn project(
        self,
        value: &mut Value,
        model: &str,
        output: OutputTokenParameter,
        effort: Option<ThinkingLevel>,
        levels: Option<&ReasoningCapabilities>,
    ) -> Result<()> {
        let object = value
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("model request must be an object"))?;
        let limit = object
            .remove("max_completion_tokens")
            .or_else(|| object.remove("max_tokens"));
        object.remove("max_tokens");
        object.remove("reasoning_effort");
        object.remove("enable_thinking");
        if let Some(limit) = limit {
            object.insert(
                match self.output_parameter(model, output) {
                    OutputTokenParameter::MaxTokens => "max_tokens",
                    _ => "max_completion_tokens",
                }
                .into(),
                limit,
            );
        }
        if let Some(effort) = effort {
            ensure!(
                levels.is_some_and(|c| c.levels.contains(&effort)),
                "Thinking level is not supported for this model/provider"
            );
            if self.0 == AdapterKind::Siliconflow {
                object.insert("enable_thinking".into(), effort.enabled().into());
                if effort.enabled() {
                    object.insert("reasoning_effort".into(), effort.reasoning_effort().into());
                }
            } else {
                object.insert("reasoning_effort".into(), effort.reasoning_effort().into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_selection_honors_provider_and_explicit_override() {
        for model in ["gpt-5.6-sol", "gpt-6.1-sol", "o3"] {
            assert_eq!(
                Adapter(AdapterKind::Openai).protocol(model, WireProtocol::Auto),
                WireProtocol::Responses
            );
            assert_eq!(
                Adapter(AdapterKind::Generic).protocol(model, WireProtocol::Auto),
                WireProtocol::ChatCompletions
            );
            assert_eq!(
                Adapter(AdapterKind::Openai).protocol(model, WireProtocol::ChatCompletions),
                WireProtocol::ChatCompletions
            );
        }
        assert_eq!(
            Adapter(AdapterKind::Generic).protocol("custom", WireProtocol::Responses),
            WireProtocol::Responses
        );
        assert_eq!(
            Adapter(AdapterKind::Siliconflow).protocol("deepseek", WireProtocol::Auto),
            WireProtocol::ChatCompletions
        );
    }
    #[test]
    fn provider_projection_is_specific_and_exclusive() {
        for (kind, model, key) in [
            (AdapterKind::Openai, "gpt-5.6-sol", "max_completion_tokens"),
            (
                AdapterKind::Siliconflow,
                "deepseek-ai/DeepSeek-V4-Flash",
                "max_tokens",
            ),
        ] {
            let a = Adapter(kind);
            let caps = a.reasoning(model, None).unwrap();
            for effort in caps.levels.iter().copied() {
                let mut v = serde_json::json!({"max_tokens":123,"messages":[]});
                a.project(
                    &mut v,
                    model,
                    OutputTokenParameter::Auto,
                    Some(effort),
                    Some(&caps),
                )
                .unwrap();
                assert_eq!(v[key], 123);
                assert!(
                    v.get(if key == "max_tokens" {
                        "max_completion_tokens"
                    } else {
                        "max_tokens"
                    })
                    .is_none()
                );
                if kind == AdapterKind::Siliconflow {
                    assert_eq!(v["enable_thinking"], effort.enabled());
                    if !effort.enabled() {
                        assert!(v.get("reasoning_effort").is_none());
                    }
                } else {
                    assert!(v.get("enable_thinking").is_none());
                    assert_eq!(v["reasoning_effort"], effort.reasoning_effort());
                }
            }
        }
    }
    #[test]
    fn unknown_capabilities_and_endpoint_spoofing_are_conservative() {
        let a = Adapter::resolve(AdapterKind::Auto, "https://api.openai.com.evil.example/v1");
        assert_eq!(a.0, AdapterKind::Generic);
        assert!(a.reasoning("unknown", None).is_none());
        let mut v = serde_json::json!({});
        assert!(
            a.project(
                &mut v,
                "unknown",
                OutputTokenParameter::Auto,
                Some(ThinkingLevel::High),
                None
            )
            .is_err()
        );
        assert!(
            Adapter(AdapterKind::Openai)
                .reasoning("gpt-4.1-mini", None)
                .unwrap()
                .levels
                .is_empty()
        );
    }
}
