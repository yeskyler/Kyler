//! Progressive skill packages and trust records, independent of agent execution.
mod archive;
mod builtin;
mod cache;
pub mod catalog;
mod manager;
mod package;
mod storage;

use std::path::Path;

use anyhow::Result;

pub use archive::{MAX_SKILL_ARCHIVE_BYTES, package_directory};
pub use manager::{SkillManager, SkillSnapshot};
pub use package::Package;

/// Host checks for arbitrary source imports and project writes. Skill code cannot
/// construct or widen sandbox grants. Package path/symlink checks remain mandatory.
pub trait SkillFileAccess: Send + Sync {
    fn check_read(&self, base: &Path, path: &Path) -> Result<()>;
    fn check_write(&self, base: &Path, path: &Path) -> Result<()>;
    fn check_replace(&self, destination: &Path) -> Result<()> {
        self.check_write(destination, destination)
    }
}
pub struct SkillDirectory {
    pub version: String,
    pub skills: Vec<kyler_protocol::api::skills::SkillInfo>,
}

pub struct SkillResource {
    pub skill: String,
    pub fingerprint: String,
    pub path: String,
    pub bytes: Vec<u8>,
}
/// Explicit references identify configured skills, never arbitrary filesystem paths.
pub fn references(input: &str) -> Vec<String> {
    let mut names = Vec::new();
    for token in input.split_whitespace() {
        if let Some(name) = token.strip_prefix('@').or_else(|| token.strip_prefix('$')) {
            if package::safe_name(name) && !names.iter().any(|old| old == name) {
                names.push(name.to_owned());
            }
        }
    }
    names
}

/// Validate bounded references before admission or expensive filesystem work.
pub fn validate_references(
    references: &[kyler_protocol::api::skills::SkillReference],
) -> Result<()> {
    anyhow::ensure!(references.len() <= 16, "at most 16 skills may be selected");
    let mut seen = std::collections::BTreeMap::new();
    for reference in references {
        anyhow::ensure!(
            [&reference.id, &reference.fingerprint]
                .iter()
                .all(|value| value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())),
            "skill references require a 64-character ID and fingerprint"
        );
        if let Some(previous) = seen.insert(&reference.id, &reference.fingerprint) {
            anyhow::ensure!(
                previous == &reference.fingerprint,
                "conflicting versions selected for the same skill"
            );
        }
    }
    Ok(())
}
