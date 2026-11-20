//! Provider boundaries: no sessions, configuration files, tools, or runtime state.
mod adapter;
pub mod discovery;
pub mod error;
mod responses;
pub mod transport;

pub use adapter::{Adapter, AdapterKind, OutputTokenParameter, WireProtocol};

#[cfg(test)]
mod discovery_tests;
