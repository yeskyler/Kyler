//! Progressive, workspace-scoped skill loading. Skills never grant execution permissions.
mod archive;
mod builtin;
pub mod catalog;
mod loader;
mod manager;
mod storage;

pub use archive::{MAX_SKILL_ARCHIVE_BYTES, package_directory};
pub use manager::{SkillManager, SkillSnapshot};

/// Explicit references use @skill-name (or legacy $skill-name) as a complete token.
pub fn references(input: &str) -> Vec<String> {
    let mut names = Vec::new();
    for token in input.split_whitespace() {
        if let Some(name) = token.strip_prefix('@').or_else(|| token.strip_prefix('$')) {
            if loader::safe_name(name) && !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
        }
    }
    names
}
