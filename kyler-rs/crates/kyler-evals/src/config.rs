use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct EvalConfig {
    pub dataset: DatasetSettings,
    pub run: RunSettings,
    pub agent: AgentSettings,
}
impl Default for EvalConfig {
    fn default() -> Self {
        Self {
            dataset: DatasetSettings::default(),
            run: RunSettings::default(),
            agent: AgentSettings::default(),
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatasetSettings {
    pub directory: PathBuf,
    pub split: String,
}
impl Default for DatasetSettings {
    fn default() -> Self {
        Self {
            directory: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("datasets/GAIA/2023"),
            split: "validation".into(),
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunSettings {
    pub concurrency: usize,
    pub max_steps: u32,
    pub timeout_seconds: u64,
    pub results_directory: PathBuf,
}
impl Default for RunSettings {
    fn default() -> Self {
        Self {
            concurrency: 1,
            max_steps: 50,
            timeout_seconds: 600,
            results_directory: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("results"),
        }
    }
}
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentSettings {
    /// Only explicit config-file connectors are used, never the interactive connector store.
    pub enable_mcp: bool,
    pub load_user_skills: bool,
    pub enable_subagents: bool,
}
impl EvalConfig {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut value = if let Some(path) = path {
            let path = path.canonicalize()?;
            let mut value: Self = toml::from_str(&std::fs::read_to_string(&path)?)?;
            let base = path.parent().unwrap();
            for directory in [
                &mut value.dataset.directory,
                &mut value.run.results_directory,
            ] {
                if directory.is_relative() {
                    *directory = base.join(&*directory);
                }
            }
            value
        } else {
            Self::default()
        };
        // Resolve the result path before starting children or changing process directories.
        if value.run.results_directory.is_relative() {
            value.run.results_directory =
                std::env::current_dir()?.join(&value.run.results_directory);
        }
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            matches!(self.dataset.split.as_str(), "validation" | "test"),
            "split must be validation or test"
        );
        ensure!(
            (1..=32).contains(&self.run.concurrency),
            "concurrency must be in 1..=32"
        );
        ensure!(
            (1..=1000).contains(&self.run.max_steps),
            "max_steps must be in 1..=1000"
        );
        ensure!(
            (1..=86400).contains(&self.run.timeout_seconds),
            "timeout_seconds must be in 1..=86400"
        );
        Ok(())
    }
}
