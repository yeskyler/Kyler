//! Shared single-user interface preferences. Patches preserve unrelated fields.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiPreferences {
    pub last_session_id: Option<String>,
    pub sidebar_collapsed: bool,
    pub revision: u64,
}
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreferenceUpdate {
    #[serde(default, deserialize_with = "nullable_session")]
    pub last_session_id: Option<Option<String>>,
    pub sidebar_collapsed: Option<bool>,
}
fn nullable_session<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error> {
    Ok(Some(Option::<String>::deserialize(deserializer)?))
}
