use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    Pretty,
    #[default]
    Compact,
    Json,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    pub filter: String,
    pub format: LogFormat,
    pub ansi: bool,
}

impl LoggingConfig {
    pub fn development() -> Self {
        Self {
            filter: "info".into(),
            format: LogFormat::Compact,
            ansi: true,
        }
    }
    pub fn production() -> Self {
        Self::default()
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        tracing_subscriber::EnvFilter::try_new(&self.filter)
            .map_err(|_| anyhow::anyhow!("invalid logging.filter directive"))?;
        Ok(())
    }
}
impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            filter: "info".into(),
            format: LogFormat::Compact,
            ansi: false,
        }
    }
}
