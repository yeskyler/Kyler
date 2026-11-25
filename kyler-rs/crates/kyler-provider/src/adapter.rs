use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use kyler_protocol::api::models::{ModelCapabilities, ReasoningCapabilities, ThinkingLevel};

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    #[default]
    Auto,
    Openai,
    Anthropic,
    Gemini,
    Xai,
    Mistral,
    Cohere,
    Perplexity,
    Deepseek,
    Dashscope,
    Moonshot,
    Zhipu,
    Minimax,
    Volcengine,
    Hunyuan,
    Qianfan,
    Siliconflow,
    Openrouter,
    Groq,
    Together,
    Fireworks,
    Cerebras,
    Huggingface,
    AzureOpenai,
    Bedrock,
    VertexAi,
    Ollama,
    LmStudio,
    Vllm,
    LlamaCpp,
    Generic,
}
#[derive(Debug, Clone, Copy)]
pub struct ProviderPreset {
    pub kind: AdapterKind,
    pub name: &'static str,
    pub base_url: Option<&'static str>,
}

macro_rules! presets {
    ($(($kind:ident, $name:literal, $url:expr)),* $(,)?) => {
        pub const PROVIDER_PRESETS: &[ProviderPreset] = &[
            $(ProviderPreset { kind: AdapterKind::$kind, name: $name, base_url: $url }),*
        ];
    };
}
presets!(
    (Openai, "OpenAI", Some("https://api.openai.com/v1")),
    (
        Anthropic,
        "Anthropic Claude",
        Some("https://api.anthropic.com/v1")
    ),
    (
        Gemini,
        "Google Gemini",
        Some("https://generativelanguage.googleapis.com/v1beta")
    ),
    (Xai, "xAI Grok", Some("https://api.x.ai/v1")),
    (Mistral, "Mistral", Some("https://api.mistral.ai/v1")),
    (
        Cohere,
        "Cohere",
        Some("https://api.cohere.ai/compatibility/v1")
    ),
    (Perplexity, "Perplexity", Some("https://api.perplexity.ai")),
    (Deepseek, "DeepSeek", Some("https://api.deepseek.com/v1")),
    (
        Dashscope,
        "Qwen (Alibaba Cloud Model Studio)",
        Some("https://dashscope.aliyuncs.com/compatible-mode/v1")
    ),
    (
        Moonshot,
        "Kimi (Moonshot AI)",
        Some("https://api.moonshot.cn/v1")
    ),
    (
        Zhipu,
        "GLM (Zhipu AI)",
        Some("https://open.bigmodel.cn/api/paas/v4")
    ),
    (Minimax, "MiniMax", Some("https://api.minimaxi.com/v1")),
    (
        Volcengine,
        "Doubao (Volcengine Ark)",
        Some("https://ark.cn-beijing.volces.com/api/v3")
    ),
    (
        Hunyuan,
        "Tencent Hunyuan",
        Some("https://api.hunyuan.cloud.tencent.com/v1")
    ),
    (
        Qianfan,
        "Baidu Qianfan",
        Some("https://qianfan.baidubce.com/v2")
    ),
    (
        Siliconflow,
        "SiliconFlow",
        Some("https://api.siliconflow.cn/v1")
    ),
    (
        Openrouter,
        "OpenRouter",
        Some("https://openrouter.ai/api/v1")
    ),
    (Groq, "Groq", Some("https://api.groq.com/openai/v1")),
    (Together, "Together AI", Some("https://api.together.xyz/v1")),
    (
        Fireworks,
        "Fireworks AI",
        Some("https://api.fireworks.ai/inference/v1")
    ),
    (Cerebras, "Cerebras", Some("https://api.cerebras.ai/v1")),
    (
        Huggingface,
        "Hugging Face Inference Providers",
        Some("https://router.huggingface.co/v1")
    ),
    (
        AzureOpenai,
        "Azure OpenAI / Microsoft Foundry",
        Some("https://resource.openai.azure.com/openai/v1")
    ),
    (
        Bedrock,
        "AWS Bedrock (API key)",
        Some("https://bedrock-mantle.us-east-1.api.aws/v1")
    ),
    (
        VertexAi,
        "Google Vertex AI (access token)",
        Some(
            "https://aiplatform.googleapis.com/v1/projects/project-id/locations/global/endpoints/openapi"
        )
    ),
    (Ollama, "Ollama", Some("http://127.0.0.1:11434/v1")),
    (LmStudio, "LM Studio", Some("http://127.0.0.1:1234/v1")),
    (Vllm, "vLLM", Some("http://127.0.0.1:8000/v1")),
    (LlamaCpp, "llama.cpp", Some("http://127.0.0.1:8080/v1")),
    (Generic, "Custom（OpenAI Completion）", None),
);

impl AdapterKind {
    pub fn preset(self) -> &'static ProviderPreset {
        PROVIDER_PRESETS
            .iter()
            .find(|preset| preset.kind == self)
            .unwrap_or_else(|| PROVIDER_PRESETS.last().expect("custom provider preset"))
    }

    pub fn is_local(self) -> bool {
        matches!(
            self,
            Self::Ollama | Self::LmStudio | Self::Vllm | Self::LlamaCpp
        )
    }

    pub fn editable_url(self) -> bool {
        self.is_local() || self == Self::Generic || self.is_enterprise()
    }

    pub fn is_enterprise(self) -> bool {
        matches!(self, Self::AzureOpenai | Self::Bedrock | Self::VertexAi)
    }
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
        if self.0 == AdapterKind::Gemini {
            return WireProtocol::GeminiGenerateContent;
        }
        if self.0 == AdapterKind::Anthropic {
            return WireProtocol::AnthropicMessages;
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
                Some(host)
                    if host.ends_with(".openai.azure.com")
                        || host.ends_with(".services.ai.azure.com") =>
                {
                    AdapterKind::AzureOpenai
                }
                Some(host)
                    if (host.starts_with("bedrock-mantle.") && host.ends_with(".api.aws"))
                        || (host.starts_with("bedrock-runtime.")
                            && host.ends_with(".amazonaws.com")) =>
                {
                    AdapterKind::Bedrock
                }
                Some(host)
                    if host == "aiplatform.googleapis.com"
                        || host.ends_with("-aiplatform.googleapis.com") =>
                {
                    AdapterKind::VertexAi
                }
                Some(host) => PROVIDER_PRESETS
                    .iter()
                    .filter(|preset| !preset.kind.is_local())
                    .find(|preset| {
                        preset
                            .base_url
                            .and_then(|url| url::Url::parse(url).ok())
                            .is_some_and(|url| url.host_str() == Some(host))
                    })
                    .map_or(AdapterKind::Generic, |preset| preset.kind),
                None => AdapterKind::Generic,
            },
        )
    }
    /// Native DeepSeek IDs are unqualified; hosted aliases belong to their hosting connection.
    pub fn accepts_model_id(self, id: &str) -> bool {
        self.0 != AdapterKind::Deepseek || !id.contains('/')
    }
    pub fn selectable_model(self, id: &str) -> bool {
        if !self.accepts_model_id(id) {
            return false;
        }
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
            AdapterKind::Deepseek
                if matches!(
                    model,
                    "deepseek-chat"
                        | "deepseek-reasoner"
                        | "deepseek-flash"
                        | "deepseek-pro"
                        | "deepseek-v4-flash"
                        | "deepseek-v4-pro"
                ) =>
            {
                vec![Off, Low, High, Max]
            }
            _ if explicit.is_some() => vec![explicit.unwrap()],
            _ => return None,
        };
        let default = explicit.filter(|v| levels.contains(v)).unwrap_or(
            if self.0 == AdapterKind::Siliconflow {
                High
            } else if self.0 == AdapterKind::Deepseek {
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
        matches!(self.0, AdapterKind::Siliconflow | AdapterKind::Deepseek)
    }
    /// Convert unified intent only after the host has projected its session.
    pub fn project(
        self,
        value: &mut Value,
        model: &str,
        output: OutputTokenParameter,
        effort: Option<ThinkingLevel>,
        _levels: Option<&ReasoningCapabilities>,
    ) -> Result<()> {
        let effort = effort.filter(|level| *level != ThinkingLevel::Auto);
        let object = value
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("model request must be an object"))?;
        let limit = object
            .remove("max_completion_tokens")
            .or_else(|| object.remove("max_tokens"));
        object.remove("max_tokens");
        object.remove("reasoning_effort");
        object.remove("enable_thinking");
        object.remove("thinking");
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
        if self.0 == AdapterKind::Deepseek && effort.is_some() {
            let enabled = effort.is_some_and(|level| level.enabled());
            object.insert(
                "thinking".into(),
                serde_json::json!({
                    "type": if enabled { "enabled" } else { "disabled" }
                }),
            );
        }
        if let Some(effort) = effort {
            if self.0 == AdapterKind::Deepseek {
                if effort.enabled() {
                    object.insert("reasoning_effort".into(), effort.reasoning_effort().into());
                }
            } else if self.0 == AdapterKind::Siliconflow {
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
            .is_ok()
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

impl Adapter {
    /// Resolve per-model declarations without inheriting the installation model's capabilities.
    /// Unknown models remain unknown unless metadata or explicit host configuration opts in.
    pub fn resolve_capabilities(
        self,
        model: &str,
        configured: &ModelCapabilities,
        discovered: Option<&ModelCapabilities>,
        effort: Option<ThinkingLevel>,
    ) -> Result<ModelCapabilities> {
        let inferred = self.capabilities(model, effort);
        let metadata = discovered.cloned().unwrap_or_default();
        let mut result = configured.clone();
        result.reasoning = configured
            .reasoning
            .clone()
            .map(|mut caps| {
                caps.source = "configuration".into();
                caps
            })
            .or_else(|| {
                metadata.reasoning.clone().map(|mut caps| {
                    caps.source = "provider".into();
                    caps
                })
            })
            .or(inferred.reasoning);
        result.images = result.images.or(metadata.images).or(inferred.images);
        result.tools = result.tools.or(metadata.tools).or(inferred.tools);
        result.structured_output = result
            .structured_output
            .or(metadata.structured_output)
            .or(inferred.structured_output);
        result.max_context_tokens = result
            .max_context_tokens
            .or(metadata.max_context_tokens)
            .or(inferred.max_context_tokens);
        result.max_output_tokens = result
            .max_output_tokens
            .or(metadata.max_output_tokens)
            .or(inferred.max_output_tokens);
        if let (Some(reasoning), Some(effort)) = (&mut result.reasoning, effort) {
            reasoning.default = effort;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod capability_resolution_tests {
    use super::*;
    #[test]
    fn configuration_metadata_and_adapter_rules_have_per_model_precedence() {
        let adapter = Adapter(AdapterKind::Openai);
        let metadata = ModelCapabilities {
            reasoning: Some(ReasoningCapabilities {
                levels: vec![ThinkingLevel::High, ThinkingLevel::Ultra],
                default: ThinkingLevel::High,
                source: "untrusted".into(),
            }),
            ..Default::default()
        };
        let resolved = adapter
            .resolve_capabilities("gpt-5.6-sol", &Default::default(), Some(&metadata), None)
            .unwrap();
        assert_eq!(
            resolved.reasoning.unwrap().levels,
            vec![ThinkingLevel::High, ThinkingLevel::Ultra]
        );
        let explicit = ModelCapabilities {
            reasoning: Some(ReasoningCapabilities {
                levels: vec![],
                default: ThinkingLevel::Off,
                source: "fixture".into(),
            }),
            ..Default::default()
        };
        assert!(
            adapter
                .resolve_capabilities("gpt-5.6-sol", &explicit, Some(&metadata), None)
                .unwrap()
                .reasoning
                .unwrap()
                .levels
                .is_empty()
        );
        assert!(
            Adapter(AdapterKind::Generic)
                .resolve_capabilities("gpt-5.6-sol", &Default::default(), None, None)
                .unwrap()
                .reasoning
                .is_none()
        );
        assert!(
            !adapter
                .reasoning("gpt-5.6-sol", None)
                .unwrap()
                .levels
                .contains(&ThinkingLevel::Ultra)
        );
    }
    #[test]
    fn ultra_is_never_silently_projected_as_max() {
        let adapter = Adapter(AdapterKind::Generic);
        let caps = ReasoningCapabilities {
            levels: vec![ThinkingLevel::Max, ThinkingLevel::Ultra],
            default: ThinkingLevel::Max,
            source: "configuration".into(),
        };
        let mut request = serde_json::json!({});
        adapter
            .project(
                &mut request,
                "custom",
                OutputTokenParameter::Auto,
                Some(ThinkingLevel::Ultra),
                Some(&caps),
            )
            .unwrap();
        assert_eq!(request["reasoning_effort"], "ultra");
        let caps = ReasoningCapabilities {
            levels: vec![ThinkingLevel::Max],
            ..caps
        };
        assert!(
            adapter
                .project(
                    &mut request,
                    "custom",
                    OutputTokenParameter::Auto,
                    Some(ThinkingLevel::Ultra),
                    Some(&caps)
                )
                .is_ok()
        );
    }
}

#[cfg(test)]
mod auto_thinking_tests {
    use super::*;

    #[test]
    fn auto_omits_all_provider_thinking_controls() {
        for kind in [
            AdapterKind::Openai,
            AdapterKind::Deepseek,
            AdapterKind::Siliconflow,
            AdapterKind::Generic,
        ] {
            let mut request = serde_json::json!({"reasoning_effort":"high","enable_thinking":true,"thinking":{"type":"enabled"}});
            Adapter(kind)
                .project(
                    &mut request,
                    "custom",
                    OutputTokenParameter::Auto,
                    Some(ThinkingLevel::Auto),
                    None,
                )
                .unwrap();
            for key in ["reasoning_effort", "enable_thinking", "thinking"] {
                assert!(request.get(key).is_none(), "{kind:?}: {key}");
            }
        }
    }

    #[test]
    fn missing_or_mismatched_thinking_metadata_does_not_block_resolution() {
        let adapter = Adapter(AdapterKind::Generic);
        assert!(
            adapter
                .resolve_capabilities("unknown", &Default::default(), None, None)
                .unwrap()
                .reasoning
                .is_none()
        );
        let configured = ModelCapabilities {
            reasoning: Some(ReasoningCapabilities {
                levels: vec![ThinkingLevel::High],
                default: ThinkingLevel::Medium,
                source: "configuration".into(),
            }),
            ..Default::default()
        };
        assert!(
            adapter
                .resolve_capabilities("custom", &configured, None, Some(ThinkingLevel::Ultra))
                .is_ok()
        );
        assert_eq!(
            Adapter(AdapterKind::Deepseek)
                .reasoning("deepseek-flash", None)
                .unwrap()
                .default,
            ThinkingLevel::High
        );
    }
}
