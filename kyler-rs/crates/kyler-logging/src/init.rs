use crate::{
    config::{LogFormat, LoggingConfig},
    filter::resolve_filter,
};

use tracing::Dispatch;

/// Builds a subscriber without changing global state. All formats write to stderr.
pub fn dispatch(config: &LoggingConfig) -> anyhow::Result<Dispatch> {
    config.validate()?;
    let filter = resolve_filter(config)?;
    Ok(match config.format {
        LogFormat::Pretty => Dispatch::new(
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .with_ansi(config.ansi)
                .pretty()
                .finish(),
        ),
        LogFormat::Compact => Dispatch::new(
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .with_ansi(config.ansi)
                .compact()
                .finish(),
        ),
        LogFormat::Json => Dispatch::new(
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .json()
                .finish(),
        ),
    })
}

/// Call once at the application entry point, before starting workers. Core never calls this.
/// A previously installed global subscriber is an error; it is not silently ignored.
pub fn init(config: LoggingConfig) -> anyhow::Result<()> {
    tracing::dispatcher::set_global_default(dispatch(&config)?)
        .map_err(|_| anyhow::anyhow!("global tracing subscriber already initialized"))
}
