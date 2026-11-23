//! Bundled guidance is embedded in the binary and never materialized in HOME.
use super::package::{self as loader, Package};

use std::collections::BTreeMap;

use kyler_config::SkillSettings;
use kyler_protocol::api::skills::{SkillInfo, SkillSource, SkillStatus};

/// An immutable package provided by the embedding application.
struct BuiltinSkill {
    name: &'static str,
    files: &'static [(&'static str, &'static str)],
}

const SKILLS: &[BuiltinSkill] = &[
    BuiltinSkill {
        name: "skill-creator",
        files: &[
            ("SKILL.md", include_str!("builtin/skill-creator/SKILL.md")),
            (
                "assets/skill-template.md",
                include_str!("builtin/skill-creator/assets/skill-template.md"),
            ),
            (
                "references/package-design.md",
                include_str!("builtin/skill-creator/references/package-design.md"),
            ),
        ],
    },
    BuiltinSkill {
        name: "skill-installer",
        files: &[
            ("SKILL.md", include_str!("builtin/skill-installer/SKILL.md")),
            (
                "references/git-sources.md",
                include_str!("builtin/skill-installer/references/git-sources.md"),
            ),
        ],
    },
];

pub fn packages(_settings: &SkillSettings) -> anyhow::Result<Vec<Package>> {
    SKILLS
        .iter()
        .map(|skill| {
            let text = skill
                .files
                .iter()
                .find(|(path, _)| *path == "SKILL.md")
                .map(|(_, text)| *text)
                .ok_or_else(|| anyhow::anyhow!("built-in skill is missing SKILL.md"))?;
            let (name, description, instructions) = loader::instructions(text.as_bytes())?;
            anyhow::ensure!(name == skill.name, "built-in skill name mismatch");
            let mut files = BTreeMap::new();
            for (path, text) in skill.files {
                anyhow::ensure!(
                    path.split('/').all(|part| !matches!(part, "" | "." | ".."))
                        && !path.contains('\\')
                        && !path.chars().any(char::is_control),
                    "invalid built-in skill resource path"
                );
                anyhow::ensure!(
                    files
                        .insert((*path).to_owned(), loader::digest(text.as_bytes()))
                        .is_none(),
                    "duplicate built-in skill resource"
                );
            }
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
                    resources: files
                        .keys()
                        .filter(|path| *path != "SKILL.md")
                        .cloned()
                        .collect(),
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
    SKILLS
        .iter()
        .find(|skill| skill.name == name)
        .and_then(|skill| skill.files.iter().find(|(name, _)| *name == path))
        .map(|(_, text)| text.as_bytes().to_vec())
        .ok_or_else(|| anyhow::anyhow!("unknown built-in skill resource"))
}
