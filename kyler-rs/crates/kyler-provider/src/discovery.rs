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
    let mut request = client
        .get(format!("{}/models", base_url.trim_end_matches('/')))
        .bearer_auth(api_key);
    if adapter.0 == crate::AdapterKind::Siliconflow {
        request = request.query(&[("sub_type", "chat")]);
    }
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
    let value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| anyhow::anyhow!("Invalid model discovery JSON"))?;
    let entries = value["data"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Model discovery response has no data array"))?;
    ensure!(
        entries.len() <= 10000,
        "Model directory exceeds entry limit"
    );
    let mut models = std::collections::BTreeMap::new();
    for item in entries {
        if let Some(id) = item["id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
        {
            models
                .entry(id.to_owned())
                .or_insert_with(|| DiscoveredModel {
                    id: id.to_owned(),
                    capabilities: reported_capabilities(item),
                });
        }
    }
    Ok(models.into_values().collect())
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
