use std::path::{Component, Path, PathBuf};

use anyhow::{Result, ensure};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkillSettings {
    pub enabled: bool,
    pub user_directory: PathBuf,
    pub project_directory: PathBuf,
    pub state_file: PathBuf,
    pub max_instruction_bytes: usize,
    pub max_resource_bytes: usize,
    pub catalog_token_budget: usize,
}
impl Default for SkillSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            user_directory: "~/.kyler/skills".into(),
            project_directory: ".kyler/skills".into(),
            state_file: "~/.kyler/skills-state.json".into(),
            max_instruction_bytes: 32768,
            max_resource_bytes: 131072,
            catalog_token_budget: 4096,
        }
    }
}
impl SkillSettings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.user_directory.as_os_str().is_empty() && !self.state_file.as_os_str().is_empty(),
            "skill paths cannot be empty"
        );
        ensure!(
            !self.project_directory.as_os_str().is_empty()
                && self
                    .project_directory
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "skills.project_directory must be a workspace-relative path without traversal"
        );
        ensure!(
            (1024..=262144).contains(&self.max_instruction_bytes)
                && (1024..=1048576).contains(&self.max_resource_bytes),
            "skill file limits are outside supported bounds"
        );
        ensure!(
            (256..=16384).contains(&self.catalog_token_budget),
            "skills.catalog_token_budget must be in 256..=16384"
        );
        Ok(())
    }
    pub fn resolve_paths(&mut self, base: &Path, home: Option<&Path>) -> Result<()> {
        for path in [&mut self.user_directory, &mut self.state_file] {
            if let Ok(rest) = path.strip_prefix("~") {
                *path = if let Some(home) = home {
                    home.join(rest)
                } else {
                    base.join(
                        path.file_name()
                            .ok_or_else(|| anyhow::anyhow!("invalid skill path"))?,
                    )
                };
            } else if path.is_relative() {
                *path = base.join(&*path);
            }
        }
        Ok(())
    }
}
