//! Explicit, project-scoped long-term facts. No automatic extraction of secrets.
use crate::{
    approval::ToolOperation,
    memory::MemoryStore,
    tools::{Tool, ToolContext, ToolExecutionMode},
};

use std::sync::Arc;

use anyhow::Result;
use serde_json::{Value, json};

pub struct MemoryTool {
    pub store: Arc<MemoryStore>,
    pub scope: String,
}
#[async_trait::async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }
    fn description(&self) -> &str {
        "Search, explicitly save, update or delete user-approved project facts and preferences across sessions. Do not save credentials, transient task progress or unverified claims. Use expected_revision for conflict detection."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"action":{"type":"string","enum":["search","save","delete"]},"query":{"type":"string"},"key":{"type":"string"},"content":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}},"expected_revision":{"type":"integer"}},"required":["action"],"additionalProperties":false})
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Host
    }
    fn approval_requirement(&self, args: &Value) -> Result<ToolOperation> {
        Ok(if args["action"] == "search" {
            ToolOperation::Read
        } else {
            ToolOperation::Write
        })
    }
    async fn execute(&self, args: &str, context: &mut ToolContext<'_>) -> Result<String> {
        let args: Value = serde_json::from_str(args)?;
        let store = self.store.clone();
        let scope = self.scope.clone();
        let session = context.session_id.to_owned();
        tokio::task::spawn_blocking(move||{let value=match args["action"].as_str(){
            Some("search")=>serde_json::to_value(store.search(&scope,args["query"].as_str().unwrap_or_default())?)?,
            Some("save")=>serde_json::to_value(store.put(&scope,args["key"].as_str().unwrap_or_default(),args["content"].as_str().unwrap_or_default(),args["tags"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect(),args["expected_revision"].as_u64(),&session)?)?,
            Some("delete")=>json!({"deleted":store.delete(&scope,args["key"].as_str().unwrap_or_default())?}),
            _=>anyhow::bail!("Invalid memory action"),
        };Ok(serde_json::to_string(&value)?)}).await?
    }
}
