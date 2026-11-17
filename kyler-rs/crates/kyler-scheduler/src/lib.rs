//! Durable time-based task orchestration, independent of HTTP and agent execution.
pub mod executor;
pub mod schedule;
mod scheduler;
mod store;

pub use scheduler::{Scheduler, SchedulerHandle};
pub use store::Store;

use std::fmt;

#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Conflict(String),
    NotFound,
    LeaseLost,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) | Self::Conflict(message) => f.write_str(message),
            Self::NotFound => f.write_str("Automation was not found."),
            Self::LeaseLost => f.write_str("Scheduler lease is unavailable."),
        }
    }
}
impl std::error::Error for Error {}
pub(crate) fn invalid(message: impl Into<String>) -> anyhow::Error {
    Error::Invalid(message.into()).into()
}
pub(crate) fn conflict(message: impl Into<String>) -> anyhow::Error {
    Error::Conflict(message.into()).into()
}

#[cfg(test)]
mod tests;
