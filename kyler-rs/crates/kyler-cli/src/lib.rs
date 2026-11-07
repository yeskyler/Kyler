//! Command-line application. Execution and persistence belong to kyler-server.
pub mod args;
mod commands;
mod output;

use std::{fmt, io};

use kyler_client::ErrorKind;

pub use commands::execute;

pub type Result<T> = std::result::Result<T, CliError>;
#[derive(Debug)]
pub struct CliError {
    pub code: u8,
    pub message: String,
}
impl CliError {
    pub fn new(code: u8, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn io(error: io::Error) -> Self {
        Self::new(
            if error.kind() == io::ErrorKind::BrokenPipe {
                0
            } else {
                3
            },
            error.to_string(),
        )
    }
}
impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CliError {}
impl From<kyler_client::Error> for CliError {
    fn from(error: kyler_client::Error) -> Self {
        let code = if error.kind == ErrorKind::InvalidInput {
            2
        } else {
            3
        };
        Self::new(code, error.to_string())
    }
}
