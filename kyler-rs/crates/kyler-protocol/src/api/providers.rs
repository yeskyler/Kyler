//! Provider configuration contracts. Responses never include credentials.
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct ProviderPreset {
    pub adapter: String,
    pub name: String,
    pub base_url: Option<String>,
    pub key_required: bool,
}
#[derive(Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: String,
    pub adapter: String,
    pub base_url: String,
    pub key_configured: bool,
    pub models: Vec<String>,
    pub default_model: Option<String>,
    pub status: String,
}
#[derive(Serialize, Deserialize)]
pub struct ProviderCatalog {
    pub presets: Vec<ProviderPreset>,
    pub providers: Vec<ProviderInfo>,
}
// Deliberately no Debug implementation: requests can contain a secret.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderUpdate {
    pub adapter: String,
    pub base_url: String,
    pub key: Option<String>,
    pub models: Vec<String>,
    #[serde(default)]
    pub make_default: bool,
}
#[derive(Serialize, Deserialize)]
pub struct ProviderTest {
    pub models: Vec<String>,
}
