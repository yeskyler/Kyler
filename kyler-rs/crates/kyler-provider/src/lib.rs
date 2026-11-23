//! Provider boundaries: no sessions, configuration files, tools, or runtime state.
mod adapter;
mod anthropic;
pub mod discovery;
pub mod error;
mod gemini;
mod responses;
pub mod transport;

pub use adapter::{
    Adapter, AdapterKind, OutputTokenParameter, PROVIDER_PRESETS, ProviderPreset, WireProtocol,
};

#[cfg(test)]
mod discovery_tests;

#[cfg(test)]
mod native_tests;
