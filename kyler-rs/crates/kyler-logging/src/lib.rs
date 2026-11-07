//! Application-owned tracing setup. Runtime libraries emit events without global initialization.
pub mod config;
pub mod filter;
pub mod init;

pub use config::{LogFormat, LoggingConfig};
pub use init::{dispatch, init};

/// Compatibility path for tracing initialization.
pub use init as log_init;
