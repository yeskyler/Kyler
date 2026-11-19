//! Task-local MCP activation. Retrieval never grants execution authority.
use super::Agent;
use crate::{
    context::{ContentItem, ContextCompactor},
    session::Session,
    tools::{
        ToolSource,
        search_tool_bm25::{Activation, SearchToolBm25, choose},
    },
};

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{Result, ensure};
use serde_json::json;
use sha2::{Digest, Sha256};

use kyler_mcp::search::{ToolIndex, ToolMetadata};
use kyler_protocol::sandbox::SandboxMode;

#[derive(Default)]
pub(super) struct McpRetrieval {
    key: Option<(String, Vec<String>)>,
    index: Option<Arc<ToolIndex>>,
    costs: Arc<HashMap<String, usize>>,
    activation: Arc<Mutex<Activation>>,
}
impl McpRetrieval {
    pub(super) fn note_calls(
        &self,
        calls: &[async_openai::types::chat::ChatCompletionMessageToolCalls],
    ) {
        let mut state = self.activation.lock().unwrap();
        let used: Vec<_> = calls
            .iter()
            .filter_map(|call| {
                if let async_openai::types::chat::ChatCompletionMessageToolCalls::Function(call) =
                    call
                {
                    state
                        .active
                        .contains(&call.function.name)
                        .then(|| call.function.name.clone())
                } else {
                    None
                }
            })
            .collect();
        for name in used.into_iter().rev() {
            state.active.retain(|old| old != &name);
            state.active.insert(0, name);
        }
    }
}
pub(super) fn task_input(session: &Session) -> String {
    session
        .events
        .get(session.memory.current_question_index)
        .and_then(|event| {
            event.content.iter().find_map(|item| {
                if let ContentItem::Message { content, .. } = item {
                    Some(content.clone())
                } else {
                    None
                }
            })
        })
        .unwrap_or_default()
}
impl Agent {
    pub(super) fn mcp_tools_allowed(&self) -> bool {
        let (policy, effective) = self.permission_environment();
        let read_only = policy
            .as_ref()
            .is_some_and(|p| p.mode == kyler_protocol::api::permissions::PermissionMode::ReadOnly)
            || self
                .environment
                .as_ref()
                .is_some_and(|e| e.policy().mode == SandboxMode::ReadOnly);
        !read_only
            && (self.environment.as_ref().map(|e| e.policy().mode)
                == effective.as_ref().map(|e| e.policy().mode)
                || (policy.is_some_and(|p| {
                    p.mode == kyler_protocol::api::permissions::PermissionMode::FullAccess
                }) && self
                    .environment
                    .as_ref()
                    .is_some_and(|e| e.allows_full_access())))
    }
    pub(super) async fn select_mcp_tools(
        &mut self,
        session: &Session,
        retrieval: &mut McpRetrieval,
        revision: &str,
    ) -> Result<()> {
        if !self.mcp_tools_allowed() {
            retrieval.key = None;
            retrieval.index = None;
            retrieval.activation.lock().unwrap().active.clear();
            Arc::make_mut(&mut self.toolbox).retain(|name, tool| {
                name != "search_tool_bm25" && !matches!(tool.source(), ToolSource::Mcp { .. })
            });
            return Ok(());
        }
        let settings = self
            .mcp
            .as_ref()
            .map(|manager| manager.search_settings())
            .unwrap_or_default();
        if !settings.enabled {
            return Ok(());
        }
        let mut remote: Vec<_> = self
            .toolbox
            .values()
            .filter(|tool| matches!(tool.source(), ToolSource::Mcp { .. }))
            .cloned()
            .collect();
        let servers = if let Some(manager) = &self.mcp {
            manager
                .connection_states(
                    self.mcp_scope.as_deref().unwrap_or(&session.session_id),
                    self.environment.clone(),
                )
                .await?
                .into_iter()
                .map(|(id, status)| json!({"id":id,"status":status}))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if remote.is_empty() && servers.is_empty() {
            retrieval.key = None;
            retrieval.index = None;
            retrieval.activation.lock().unwrap().active.clear();
            return Ok(());
        }
        remote.sort_by(|a, b| a.name().cmp(b.name()));
        let names = remote
            .iter()
            .map(|tool| tool.name().to_owned())
            .collect::<Vec<_>>();
        let key = (revision.to_owned(), names);
        if retrieval.key.as_ref() != Some(&key) {
            let counter = ContextCompactor::new(self.context_policy.clone())?.with_tokenizer(
                self.model_info()
                    .map(|model| model.capabilities.tokenizer)
                    .unwrap_or_default(),
            );
            // Build new immutable metadata off the async executor; swap only when complete.
            let (index, costs) = tokio::task::spawn_blocking(move || {
                let mut documents = Vec::new();
                let mut costs = HashMap::new();
                for tool in &remote {
                    let ToolSource::Mcp {
                        server_id,
                        remote_name,
                    } = tool.source()
                    else {
                        continue;
                    };
                    let mut definition = tool.definition()?;
                    crate::approval::policy::advertise(&mut definition.parameters)?;
                    let cost = counter.count(&[crate::models::project_tool(definition)?])?;
                    costs.insert(tool.name().to_owned(), cost);
                    documents.push(ToolMetadata {
                        name: tool.name().into(),
                        server_id,
                        remote_name,
                        description: tool.description().into(),
                        parameters: tool.parameters(),
                        version: format!(
                            "{:x}",
                            Sha256::digest(tool.approval_identity().as_bytes())
                        ),
                    });
                }
                let index = Arc::new(ToolIndex::new(documents)?);
                Ok::<_, anyhow::Error>((index, costs))
            })
            .await??;
            let old = retrieval.index.as_ref();
            let previous = retrieval
                .activation
                .lock()
                .unwrap()
                .active
                .iter()
                .filter(|name| {
                    old.is_some_and(|old| {
                        old.documents()
                            .iter()
                            .find(|doc| &doc.name == *name)
                            .is_some_and(|prior| {
                                index.documents().iter().any(|doc| {
                                    doc.name == prior.name && doc.version == prior.version
                                })
                            })
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            retrieval.activation.lock().unwrap().active = choose(previous, &costs, &settings);
            retrieval.costs = Arc::new(costs);
            retrieval.index = Some(index);
            retrieval.key = Some(key);
        }
        let index = retrieval
            .index
            .as_ref()
            .expect("MCP index initialized")
            .clone();
        let active = retrieval.activation.lock().unwrap().active.clone();
        let tools = Arc::make_mut(&mut self.toolbox);
        tools.retain(|name, tool| {
            !matches!(tool.source(), ToolSource::Mcp { .. }) || active.contains(name)
        });
        ensure!(
            !tools.contains_key("search_tool_bm25"),
            "reserved tool name search_tool_bm25 conflicts with a configured tool"
        );
        tools.insert(
            "search_tool_bm25".into(),
            Arc::new(SearchToolBm25 {
                index: index.clone(),
                activation: retrieval.activation.clone(),
                costs: retrieval.costs.clone(),
                settings,
                servers,
            }),
        );
        tracing::debug!(catalog_version=%index.version, available_tools=index.documents().len(),active_tools=active.len(),schema_tokens=active.iter().filter_map(|name|retrieval.costs.get(name)).sum::<usize>(),"MCP tools selected");
        Ok(())
    }
}
