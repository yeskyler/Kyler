//! Reusable HTTP/SSE client. No terminal, global logger, or model credentials required.
pub mod client;
mod decoder;
pub mod error;
pub mod stream;

pub use client::Client;
pub use error::{Error, ErrorKind, Result};
pub use stream::{ClientEvent, RunStream};
