use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

pub const MAX_SKILL_ARCHIVE_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    Builtin,
    User,
    Project,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillStatus {
    Enabled,
    Disabled,
    NeedsApproval,
    Shadowed,
    Invalid,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    pub path: String,
    pub fingerprint: String,
    pub status: SkillStatus,
    pub resources: Vec<String>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDetails {
    pub info: SkillInfo,
    pub instructions: String,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillAction {
    Enable,
    Disable,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillActionRequest {
    pub fingerprint: String,
}

/// Complete UTF-8 package. Updates require the currently installed fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveSkillRequest {
    pub name: String,
    #[serde(default)]
    pub scope: SkillScope,
    pub files: BTreeMap<String, String>,
    pub expected_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    #[default]
    User,
    Project,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallSkillRequest {
    pub path: PathBuf,
    #[serde(default)]
    pub scope: SkillScope,
    pub expected_fingerprint: Option<String>,
}
