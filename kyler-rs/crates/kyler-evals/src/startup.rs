mod driver;
pub mod probe;
pub mod profile;
mod service;
pub mod statistics;

use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    #[default]
    ProcessCold,
    SessionCold,
    SessionWarm,
    SessionRestore,
    Runtime,
    Server,
    FirstTurn,
}
impl Scenario {
    pub fn argument(self) -> &'static str {
        match self {
            Self::ProcessCold => "process-cold",
            Self::SessionCold => "session-cold",
            Self::SessionWarm => "session-warm",
            Self::SessionRestore => "session-restore",
            Self::Runtime => "runtime",
            Self::Server => "server",
            Self::FirstTurn => "first-turn",
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    #[default]
    Minimal,
    Default,
    Skills,
    Mcp,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelMode {
    #[default]
    Mock,
    Live,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub scenario: Scenario,
    pub model_mode: ModelMode,
    pub repeat: usize,
    pub warmup: usize,
    pub timeout_seconds: u64,
    pub profile: Profile,
    pub skill_count: usize,
    pub skill_instruction_bytes: usize,
    pub mcp_count: usize,
    pub mcp_delay_ms: u64,
    pub required_mcp: Vec<String>,
    pub restore_turns: usize,
    pub restore_snapshot: bool,
    pub kyler_bin: Option<PathBuf>,
    /// A matching baseline executable enables alternating baseline/candidate samples.
    pub baseline_bin: Option<PathBuf>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            scenario: Scenario::ProcessCold,
            model_mode: ModelMode::Mock,
            repeat: 30,
            warmup: 2,
            timeout_seconds: 120,
            profile: Profile::Minimal,
            skill_count: 16,
            skill_instruction_bytes: 2048,
            mcp_count: 2,
            mcp_delay_ms: 50,
            required_mcp: Vec::new(),
            restore_turns: 20,
            restore_snapshot: true,
            kyler_bin: None,
            baseline_bin: None,
        }
    }
}
impl Settings {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let settings = if let Some(path) = path {
            let path = path.canonicalize()?;
            let mut settings: Self = toml::from_str(&std::fs::read_to_string(&path)?)?;
            for executable in [&mut settings.kyler_bin, &mut settings.baseline_bin]
                .into_iter()
                .flatten()
            {
                if executable.is_relative() {
                    *executable = path.parent().unwrap().join(&*executable);
                }
            }
            settings
        } else {
            Self::default()
        };
        Ok(settings)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=1000).contains(&self.repeat) && self.warmup <= 100,
            "Invalid repeat/warmup count"
        );
        ensure!(
            self.skill_count <= 128 && (256..=32768).contains(&self.skill_instruction_bytes),
            "Invalid synthetic skill size/count"
        );
        ensure!(
            self.mcp_count <= 16 && self.mcp_delay_ms <= 5000,
            "Invalid MCP fixture count/delay"
        );
        ensure!(
            self.profile != Profile::Skills || self.skill_count > 0,
            "Skills profile requires skill_count > 0"
        );
        ensure!(
            self.profile != Profile::Mcp || self.mcp_count > 0,
            "MCP profile requires mcp_count > 0"
        );
        ensure!(
            self.required_mcp.len() <= 32
                && self.required_mcp.iter().all(|id| !id.is_empty()
                    && id.len() <= 24
                    && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')),
            "Invalid required MCP connector IDs"
        );
        ensure!(
            (1..=500).contains(&self.restore_turns),
            "restore_turns must be in 1..=500"
        );
        ensure!(
            self.baseline_bin.is_none() || self.scenario == Scenario::ProcessCold,
            "Alternating executable comparison currently requires process-cold"
        );
        ensure!(
            (1..=3600).contains(&self.timeout_seconds),
            "Startup timeout must be in 1..=3600"
        );
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub index: usize,
    #[serde(default = "candidate")]
    pub variant: String,
    pub warmup: bool,
    pub status: String,
    pub process_to_ready_ms: Option<f64>,
    pub process_total_ms: f64,
    #[serde(default)]
    pub timeline_ms: std::collections::BTreeMap<String, f64>,
    #[serde(default)]
    pub build_profile: Option<String>,
    pub measurement: Option<probe::Measurement>,
    pub error: Option<String>,
}
fn candidate() -> String {
    "candidate".into()
}

pub use driver::run;
