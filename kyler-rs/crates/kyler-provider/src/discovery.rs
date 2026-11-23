//! Credential-free discovery results; callers own cache storage and refresh policy.
use std::time::Duration;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "undeclared")]
    pub capabilities: kyler_protocol::api::models::ModelCapabilities,
}
fn undeclared(caps: &kyler_protocol::api::models::ModelCapabilities) -> bool {
    caps == &Default::default()
}
/// Never follow redirects with provider credentials or include response bodies in errors.
pub async fn list_models(
    base_url: &str,
    api_key: &str,
    adapter: crate::Adapter,
) -> Result<Vec<DiscoveredModel>> {
    // Native trust-store initialization can block on some hosts; keep it off runtime workers.
    let client = tokio::task::spawn_blocking(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
    })
    .await
    .map_err(|_| anyhow::anyhow!("Model discovery client initialization failed"))?
    .map_err(|_| anyhow::anyhow!("Model discovery client initialization failed"))?;
    let gemini = adapter.0 == crate::AdapterKind::Gemini;
    let base = base_url.trim_end_matches('/');
    let base = if gemini {
        base.trim_end_matches("/openai")
    } else {
        base
    };
    let mut request = client.get(format!("{base}/models"));
    match adapter.0 {
        crate::AdapterKind::Gemini => request = request.header("x-goog-api-key", api_key),
        crate::AdapterKind::Anthropic => {
            request = request
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01");
        }
        crate::AdapterKind::AzureOpenai => request = request.header("api-key", api_key),
        _ if !api_key.is_empty() => request = request.bearer_auth(api_key),
        _ => {}
    }
    if adapter.0 == crate::AdapterKind::Siliconflow {
        request = request.query(&[("sub_type", "chat")]);
    }
    let mut cursor = None;
    let mut entries = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    for _ in 0..20 {
        let mut page = request
            .try_clone()
            .expect("model list requests have no body");
        if let Some((parameter, value)) = &cursor {
            page = page.query(&[(*parameter, value)]);
        }
        let value = tokio::time::timeout_at(deadline, discovery_response(page))
            .await
            .map_err(|_| anyhow::anyhow!("Model discovery timed out"))??;
        let field = if gemini { "models" } else { "data" };
        entries.extend(
            value[field]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Model discovery response has no model list"))?
                .iter()
                .cloned(),
        );
        ensure!(
            entries.len() <= 10000,
            "Model directory exceeds entry limit"
        );
        let next = if gemini {
            value["nextPageToken"]
                .as_str()
                .map(|token| ("pageToken", token.to_owned()))
        } else if adapter.0 == crate::AdapterKind::Anthropic && value["has_more"] == true {
            value["last_id"]
                .as_str()
                .map(|id| ("after_id", id.to_owned()))
        } else {
            None
        };
        if next.is_none() {
            cursor = None;
            break;
        }
        ensure!(next != cursor, "Model discovery cursor did not advance");
        cursor = next;
    }
    ensure!(cursor.is_none(), "Model discovery exceeds page limit");
    let mut models = std::collections::BTreeMap::new();
    for item in &entries {
        let id = if gemini {
            if !item["supportedGenerationMethods"]
                .as_array()
                .is_some_and(|methods| methods.iter().any(|method| method == "generateContent"))
            {
                continue;
            }
            item["name"]
                .as_str()
                .and_then(|name| name.strip_prefix("models/"))
        } else {
            item["id"].as_str()
        };
        if let Some(id) =
            id.filter(|id| !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
        {
            models
                .entry(id.to_owned())
                .or_insert_with(|| DiscoveredModel {
                    id: id.to_owned(),
                    capabilities: if gemini {
                        kyler_protocol::api::models::ModelCapabilities {
                            max_context_tokens: item["inputTokenLimit"]
                                .as_u64()
                                .and_then(|value| usize::try_from(value).ok())
                                .filter(|value| *value > 0),
                            max_output_tokens: item["outputTokenLimit"]
                                .as_u64()
                                .and_then(|value| u32::try_from(value).ok())
                                .filter(|value| *value > 0),
                            ..Default::default()
                        }
                    } else {
                        reported_capabilities(item)
                    },
                });
        }
    }
    Ok(models.into_values().collect())
}

async fn discovery_response(request: reqwest::RequestBuilder) -> Result<serde_json::Value> {
    let mut response = request
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Model discovery connection failed"))?;
    ensure!(
        response.status().is_success(),
        "Model discovery returned HTTP {}",
        response.status().as_u16()
    );
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("Model discovery response failed"))?
    {
        ensure!(
            body.len() + chunk.len() <= 2 * 1024 * 1024,
            "Model discovery response exceeds size limit"
        );
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| anyhow::anyhow!("Invalid model discovery JSON"))
}

/// Accept only an explicit, validated level declaration. A boolean `true` cannot
/// establish which request values are legal. Provider-controlled source text is discarded.
fn reported_capabilities(
    item: &serde_json::Value,
) -> kyler_protocol::api::models::ModelCapabilities {
    use kyler_protocol::api::models::{ModelCapabilities, ReasoningCapabilities, ThinkingLevel};
    let mut caps = ModelCapabilities::default();
    let value = item
        .pointer("/capabilities/reasoning")
        .or_else(|| item.get("reasoning"));
    caps.reasoning = value.and_then(|value| {
        if value == &serde_json::Value::Bool(false) {
            return Some(ReasoningCapabilities {
                levels: vec![],
                default: ThinkingLevel::Off,
                source: "provider".into(),
            });
        }
        let levels: Vec<ThinkingLevel> =
            serde_json::from_value(value.get("levels")?.clone()).ok()?;
        if levels.len() > ThinkingLevel::ALL.len()
            || levels
                .iter()
                .enumerate()
                .any(|(i, level)| levels[..i].contains(level))
        {
            return None;
        }
        let default = match value.get("default") {
            Some(default) => serde_json::from_value(default.clone()).ok()?,
            None => levels.first().copied().unwrap_or(ThinkingLevel::Off),
        };
        if (!levels.is_empty() && !levels.contains(&default))
            || (levels.is_empty() && default != ThinkingLevel::Off)
        {
            return None;
        }
        Some(ReasoningCapabilities {
            levels,
            default,
            source: "provider".into(),
        })
    });
    caps
}

#[cfg(test)]
mod capability_tests {
    use super::*;
    use kyler_protocol::api::models::ThinkingLevel;
    #[test]
    fn capability_metadata_is_explicit_validated_and_credential_free() {
        let model = serde_json::json!({"capabilities":{"reasoning":{"levels":["off","high","ultra"],"default":"high","source":"PRIVATE_PROVIDER_TEXT"}},"api_key":"PRIVATE"});
        let caps = reported_capabilities(&model);
        let reasoning = caps.reasoning.unwrap();
        assert_eq!(
            reasoning.levels,
            vec![
                ThinkingLevel::Off,
                ThinkingLevel::High,
                ThinkingLevel::Ultra
            ]
        );
        assert_eq!(reasoning.default, ThinkingLevel::High);
        assert_eq!(reasoning.source, "provider");
        for invalid in [
            serde_json::json!({"capabilities":{"reasoning":true}}),
            serde_json::json!({"reasoning":{"levels":["high","high"]}}),
            serde_json::json!({"reasoning":{"levels":["max"],"default":"ultra"}}),
            serde_json::json!({"reasoning":{"levels":["on"]}}),
        ] {
            assert!(reported_capabilities(&invalid).reasoning.is_none());
        }
        assert!(
            reported_capabilities(&serde_json::json!({"reasoning":false}))
                .reasoning
                .unwrap()
                .levels
                .is_empty()
        );
    }
}
