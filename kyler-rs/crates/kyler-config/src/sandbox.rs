use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use serde::Deserialize;

pub use kyler_protocol::sandbox::SandboxMode;

#[derive(Debug, Clone, Deserialize)]
#[serde(from = "SandboxSettingsInput")]
pub struct SandboxSettings {
    pub mode: SandboxMode,
    pub network_access: bool,
    pub allow_commands: bool,
    pub additional_read_roots: Vec<PathBuf>,
    pub additional_write_roots: Vec<PathBuf>,
    pub protected_paths: Vec<PathBuf>,
    pub external_sandbox_confirmed: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SandboxSettingsInput {
    #[serde(default)]
    mode: SandboxMode,
    #[serde(default)]
    network_access: Option<bool>,
    #[serde(default = "commands_enabled")]
    allow_commands: bool,
    #[serde(default)]
    additional_read_roots: Vec<PathBuf>,
    #[serde(default)]
    additional_write_roots: Vec<PathBuf>,
    #[serde(default)]
    protected_paths: Vec<PathBuf>,
    #[serde(default)]
    external_sandbox_confirmed: bool,
}
fn commands_enabled() -> bool {
    true
}
impl From<SandboxSettingsInput> for SandboxSettings {
    fn from(input: SandboxSettingsInput) -> Self {
        Self {
            mode: input.mode,
            network_access: input
                .network_access
                .unwrap_or(input.mode != SandboxMode::ReadOnly),
            allow_commands: input.allow_commands,
            additional_read_roots: input.additional_read_roots,
            additional_write_roots: input.additional_write_roots,
            protected_paths: input.protected_paths,
            external_sandbox_confirmed: input.external_sandbox_confirmed,
        }
    }
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            mode: SandboxMode::WorkspaceWrite,
            network_access: true,
            allow_commands: true,
            additional_read_roots: vec![],
            additional_write_roots: vec![],
            protected_paths: vec![],
            external_sandbox_confirmed: false,
        }
    }
}
impl SandboxSettings {
    pub fn validate(&self) -> Result<()> {
        if self.mode == SandboxMode::ReadOnly {
            ensure!(
                !self.network_access,
                "read-only sandbox does not allow network access"
            );
            ensure!(
                self.additional_write_roots.is_empty(),
                "read-only sandbox does not allow additional write roots"
            );
        }
        if self.mode == SandboxMode::ExternalSandbox {
            ensure!(
                self.external_sandbox_confirmed,
                "external-sandbox requires external_sandbox_confirmed=true from the host operator"
            );
        }
        ensure!(
            self.additional_read_roots.len()
                + self.additional_write_roots.len()
                + self.protected_paths.len()
                <= 128,
            "too many sandbox paths"
        );
        for path in self
            .additional_read_roots
            .iter()
            .chain(&self.additional_write_roots)
            .chain(&self.protected_paths)
        {
            ensure!(
                !path.as_os_str().is_empty()
                    && !path
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir)),
                "sandbox paths cannot be empty or contain parent traversal"
            );
        }
        Ok(())
    }
    pub fn resolve_paths(&mut self, base: &Path, home: Option<&Path>) -> Result<()> {
        for path in self
            .additional_read_roots
            .iter_mut()
            .chain(&mut self.additional_write_roots)
        {
            if let Ok(rest) = path.strip_prefix("~") {
                *path = home
                    .ok_or_else(|| anyhow::anyhow!("HOME is required for sandbox paths"))?
                    .join(rest);
            } else if path.is_relative() {
                *path = base.join(&*path);
            }
        }
        for path in &mut self.protected_paths {
            if let Ok(rest) = path.strip_prefix("~") {
                *path = home
                    .ok_or_else(|| anyhow::anyhow!("HOME is required for sandbox paths"))?
                    .join(rest);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::SandboxSettings;

    use kyler_protocol::sandbox::SandboxMode;

    #[test]
    fn modes_require_explicit_external_confirmation_and_readonly_has_no_write_or_network_grants() {
        let mut settings = SandboxSettings::default();
        assert!(settings.validate().is_ok());
        settings.mode = SandboxMode::ReadOnly;
        settings.network_access = true;
        assert!(settings.validate().is_err());
        settings.network_access = false;
        settings.additional_write_roots.push("/tmp".into());
        assert!(settings.validate().is_err());
        settings.additional_write_roots.clear();
        settings.mode = SandboxMode::ExternalSandbox;
        assert!(settings.validate().is_err());
        settings.external_sandbox_confirmed = true;
        assert!(settings.validate().is_ok());
    }
}

#[cfg(test)]
mod path_tests {
    use super::SandboxSettings;

    use std::path::{Path, PathBuf};

    #[test]
    fn protected_paths_remain_workspace_relative_while_extra_roots_follow_config() {
        let mut settings = SandboxSettings {
            additional_write_roots: vec!["extra".into()],
            protected_paths: vec!["secrets".into(), "~/private".into()],
            ..Default::default()
        };
        settings
            .resolve_paths(Path::new("/config"), Some(Path::new("/home/user")))
            .unwrap();
        assert_eq!(
            settings.additional_write_roots,
            [PathBuf::from("/config/extra")]
        );
        assert_eq!(
            settings.protected_paths,
            [
                PathBuf::from("secrets"),
                PathBuf::from("/home/user/private")
            ]
        );
    }
}

#[cfg(test)]
mod network_defaults {
    use super::{SandboxMode, SandboxSettings};

    #[test]
    fn workspace_defaults_online_but_readonly_and_explicit_offline_are_preserved() {
        assert!(SandboxSettings::default().network_access);
        let workspace: SandboxSettings = toml::from_str("mode = 'workspace-write'").unwrap();
        assert!(workspace.network_access);
        let offline: SandboxSettings =
            toml::from_str("mode = 'workspace-write'\nnetwork_access = false").unwrap();
        assert!(!offline.network_access);
        let readonly: SandboxSettings = toml::from_str("mode = 'read-only'").unwrap();
        assert_eq!(readonly.mode, SandboxMode::ReadOnly);
        assert!(!readonly.network_access);
        readonly.validate().unwrap();
        let invalid: SandboxSettings =
            toml::from_str("mode = 'read-only'\nnetwork_access = true").unwrap();
        assert!(invalid.validate().is_err());
        assert!(toml::from_str::<SandboxSettings>("network_acess = true").is_err());
    }
}
