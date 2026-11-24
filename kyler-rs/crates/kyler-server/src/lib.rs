//! HTTP API and bounded, reconnectable SSE over Kyler's asynchronous runtime.
mod automations;
mod channels;
pub mod error;
pub mod routes;
pub mod runs;
pub mod server;
mod sessions;
pub mod sse;
pub mod state;
pub mod workspaces;

pub use server::Server;

mod threads;

mod gateway;
