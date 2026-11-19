//! MCP client lifecycle, versioned catalogs and local retrieval.
//! Hosts create authorized transports; this crate never grants sandbox permissions.
mod call;
mod catalog;
mod connection;
mod manager;
pub mod search;

pub use catalog::{ToolCatalog, ToolDescriptor, tool_alias, validate_name};
pub use connection::{Connection, TransportFactory, TransportGuard};
pub use manager::Client;
