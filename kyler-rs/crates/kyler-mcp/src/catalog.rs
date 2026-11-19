//! Immutable, validated metadata. Catalogs contain no transport or credentials.
use std::{collections::HashSet, sync::Arc};

use anyhow::{Result, ensure};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// SDK-neutral metadata. The fingerprint also covers annotations and output schemas.
#[derive(Clone, Serialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    pub metadata_fingerprint: String,
}
impl ToolDescriptor {
    pub(crate) fn from_remote(tool: rmcp::model::Tool) -> Result<Self> {
        let bytes = serde_json::to_vec(&tool)?;
        ensure!(bytes.len() <= 64 * 1024, "MCP tool schema exceeds 64 KiB");
        Ok(Self {
            name: tool.name.to_string(),
            description: tool.description.map(|text| text.to_string()),
            input_schema: Value::Object(tool.input_schema.as_ref().clone()),
            metadata_fingerprint: format!("{:x}", Sha256::digest(bytes)),
        })
    }
}

#[derive(Clone)]
pub struct ToolCatalog {
    pub version: String,
    pub tools: Vec<Arc<ToolDescriptor>>,
}
impl ToolCatalog {
    pub fn new(mut tools: Vec<ToolDescriptor>) -> Result<Self> {
        ensure!(tools.len() <= 256, "MCP tool count exceeds 256");
        let mut names = HashSet::new();
        for tool in &tools {
            validate_name(&tool.name)?;
            ensure!(
                tool.input_schema.is_object(),
                "MCP input schema must be an object"
            );
            ensure!(
                names.insert(tool.name.to_string()),
                "duplicate MCP tool name"
            );
            ensure!(
                serde_json::to_vec(tool)?.len() <= 64 * 1024,
                "MCP tool schema exceeds 64 KiB"
            );
        }
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        let version = format!("{:x}", Sha256::digest(serde_json::to_vec(&tools)?));
        let tools = tools.into_iter().map(Arc::new).collect();
        Ok(Self { version, tools })
    }
}

pub fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control),
        "invalid MCP tool name"
    );
    Ok(())
}
pub fn tool_alias(server: &str, remote: &str) -> String {
    let simple = format!("mcp__{server}__{remote}");
    if simple.len() <= 64
        && remote
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return simple;
    }
    let hash = remote.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    let prefix: String = remote
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(40usize.saturating_sub(server.len()))
        .collect();
    format!("mcp__{server}__{prefix}_{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn tool(name: &str) -> ToolDescriptor {
        ToolDescriptor::from_remote(
            serde_json::from_value(json!({"name":name,"inputSchema":{"type":"object"}})).unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn catalogs_are_order_independent_validate_duplicates_and_names() {
        let a = ToolCatalog::new(vec![tool("one"), tool("two")]).unwrap();
        let b = ToolCatalog::new(vec![tool("two"), tool("one")]).unwrap();
        assert_eq!(a.version, b.version);
        assert!(ToolCatalog::new(vec![tool("one"), tool("one")]).is_err());
        assert!(ToolCatalog::new(vec![tool("bad\nname")]).is_err());
        assert_ne!(tool_alias("s", "foo.bar"), tool_alias("s", "foo/bar"));
    }
}
