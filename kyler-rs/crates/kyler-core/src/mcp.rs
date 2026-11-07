//! Session-scoped MCP connections and remote tool adapters.
mod connection;
mod manager;
mod package;
mod process;
mod storage;
mod system;
mod tool;

pub use manager::McpManager;

/// Explicit connector mentions identify existing tools; they never grant access.
pub fn references(input: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for token in input.split_whitespace() {
        if let Some(id) = token.strip_prefix("@connector:") {
            if !id.is_empty()
                && id.len() <= 24
                && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                && !ids.iter().any(|item| item == id)
            {
                ids.push(id.to_owned());
            }
        }
    }
    ids
}
