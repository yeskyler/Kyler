use super::{Agent, stream::StreamControl};
use crate::{
    context::ContentItem, mcp, session::Session, skills::references, subagent::SubagentScope,
    tools::ToolSource,
};

use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls, FunctionCall,
};

impl Agent {
    pub(super) async fn load_requested_skills(
        &self,
        session: &mut Session,
        scope: Option<&SubagentScope>,
        stream: Option<&StreamControl>,
    ) -> anyhow::Result<()> {
        let input = session
            .events
            .get(session.memory.current_question_index)
            .and_then(|event| {
                event.content.iter().find_map(|item| match item {
                    ContentItem::Message { content, .. } => Some(content.as_str()),
                    _ => None,
                })
            })
            .unwrap_or_default();
        for id in mcp::references(input) {
            let available = self.toolbox.values().any(|tool| {
                matches!(tool.source(), ToolSource::Mcp { server_id, .. } if server_id == id)
            });
            anyhow::ensure!(
                available,
                "Requested connector {id} has no available tools under this session policy. Connect it before sending the request."
            );
        }
        let names = references(input);
        if names.is_empty() {
            return Ok(());
        }
        let snapshot = self
            .skill_snapshot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Skills are disabled or unavailable."))?;
        for name in &names {
            let snapshot = snapshot.clone();
            let name = name.clone();
            tokio::task::spawn_blocking(move || snapshot.load(&name)).await??;
        }
        let calls = names
            .into_iter()
            .map(|name| {
                ChatCompletionMessageToolCalls::Function(ChatCompletionMessageToolCall {
                    id: format!("skill-{}", uuid::Uuid::new_v4()),
                    function: FunctionCall {
                        name: "load_skill".into(),
                        arguments: serde_json::json!({"name":name}).to_string(),
                    },
                })
            })
            .collect::<Vec<_>>();
        self.record_tool_calls(session, &calls)?;
        self.checkpoint(session).await?;
        self.execute_tool_calls(session, &calls, scope, stream)
            .await
    }
}
