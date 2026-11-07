//! Bundled guidance is embedded in the binary and never materialized in HOME.
use super::loader::{self, Package};

use std::collections::BTreeMap;

use kyler_config::SkillSettings;
use kyler_protocol::api::skills::{SkillInfo, SkillSource, SkillStatus};

const SKILLS: [&str; 2] = [
    include_str!("builtin/skill-creator/SKILL.md"),
    include_str!("builtin/skill-installer/SKILL.md"),
];

pub fn packages(_settings: &SkillSettings) -> anyhow::Result<Vec<Package>> {
    SKILLS
        .iter()
        .map(|text| {
            let (name, description, instructions) = loader::instructions(text.as_bytes())?;
            let files = BTreeMap::from([("SKILL.md".into(), loader::digest(text.as_bytes()))]);
            let fingerprint = loader::digest(&serde_json::to_vec(&files)?);
            let path = format!("builtin://{name}");
            Ok(Package {
                info: SkillInfo {
                    id: loader::digest(path.as_bytes()),
                    name,
                    description,
                    source: SkillSource::Builtin,
                    path: path.clone(),
                    fingerprint,
                    status: SkillStatus::Enabled,
                    resources: vec![],
                    error: None,
                },
                root: path.into(),
                instructions,
                files,
            })
        })
        .collect()
}

pub fn resource(name: &str, path: &str) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(path == "SKILL.md", "unknown built-in resource");
    for text in SKILLS {
        if loader::instructions(text.as_bytes())?.0 == name {
            return Ok(text.as_bytes().to_vec());
        }
    }
    anyhow::bail!("unknown built-in skill")
}
