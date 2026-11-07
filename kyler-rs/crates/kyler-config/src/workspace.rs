//! Host-owned default task directory, separate from configuration and session data.
use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceSettings {
    pub default_directory: PathBuf,
}
impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            default_directory: "workspace".into(),
        }
    }
}
impl WorkspaceSettings {
    pub fn resolve_paths(&mut self, base: &Path, home: Option<&Path>) -> Result<()> {
        ensure!(
            !self.default_directory.as_os_str().is_empty(),
            "workspace.default_directory cannot be empty"
        );
        if let Ok(rest) = self.default_directory.strip_prefix("~") {
            self.default_directory = home
                .ok_or_else(|| {
                    anyhow::anyhow!("HOME is required to expand workspace.default_directory")
                })?
                .join(rest);
        } else if self.default_directory.is_relative() {
            self.default_directory = base.join(&self.default_directory);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_lives_beside_config_and_explicit_paths_are_preserved() {
        let mut settings = WorkspaceSettings::default();
        settings
            .resolve_paths(Path::new("/home/user/.kyler"), None)
            .unwrap();
        assert_eq!(
            settings.default_directory,
            Path::new("/home/user/.kyler/workspace")
        );
        settings.default_directory = "~/.kyler/tasks".into();
        settings
            .resolve_paths(Path::new("/config"), Some(Path::new("/home/user")))
            .unwrap();
        assert_eq!(
            settings.default_directory,
            Path::new("/home/user/.kyler/tasks")
        );
        settings.default_directory = "".into();
        assert!(settings.resolve_paths(Path::new("/config"), None).is_err());
    }
}
