//! Credential-free discovery results; callers own cache storage and refresh policy.
use std::time::Duration;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub id: String,
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
    let mut ids = std::collections::BTreeSet::new();
    for item in entries {
        if let Some(id) = item["id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
        {
            ids.insert(id.to_owned());
        }
    }
    Ok(ids.into_iter().map(|id| DiscoveredModel { id }).collect())
}
