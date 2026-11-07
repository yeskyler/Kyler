use crate::config::LoggingConfig;

use std::env::VarError;

use tracing_subscriber::EnvFilter;

pub fn resolve_filter(config: &LoggingConfig) -> anyhow::Result<EnvFilter> {
    let directive = match std::env::var("RUST_LOG") {
        Ok(value) => value,
        Err(VarError::NotPresent) => config.filter.clone(),
        Err(_) => anyhow::bail!("RUST_LOG must be valid Unicode"),
    };
    EnvFilter::try_new(directive)
        .map_err(|_| anyhow::anyhow!("invalid log filter; check RUST_LOG and logging.filter"))
}
