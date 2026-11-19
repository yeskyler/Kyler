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
                "Requested connector {id} has no tools available under this session policy yet. Check its connection status and permissions, then retry after discovery completes."
            );
        }
        let names = references(input);
        let mut selected = session.current_execution()?.selected_skills.clone();
        if names.is_empty() && selected.is_empty() {
            return Ok(());
        }
        let snapshot = self
            .skill_snapshot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Skills are disabled or unavailable."))?;
        for name in names {
            let package = snapshot.available.get(&name).ok_or_else(|| {
                anyhow::anyhow!("Requested skill {name} is not enabled or is shadowed.")
            })?;
            if !selected
                .iter()
                .any(|reference| reference.id == package.info.id)
            {
                selected.push(kyler_protocol::api::skills::SkillReference {
                    id: package.info.id.clone(),
                    fingerprint: package.info.fingerprint.clone(),
                });
            }
        }
        crate::skills::validate_references(&selected)?;
        let mut calls = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for reference in &selected {
            if !seen.insert(reference.id.clone()) {
                continue;
            }
            let snapshot = snapshot.clone();
            let reference = reference.clone();
            let details =
                tokio::task::spawn_blocking(move || snapshot.load_reference(&reference)).await??;
            calls.push(ChatCompletionMessageToolCalls::Function(ChatCompletionMessageToolCall {
                id:format!("skill-{}",uuid::Uuid::new_v4()),
                function:FunctionCall {name:"load_skill".into(),arguments:serde_json::json!({"name":details.info.name,"expected_fingerprint":details.info.fingerprint}).to_string()},
            }));
        }
        session.current_execution_mut()?.selected_skills = selected;
        self.record_tool_calls(session, &calls)?;
        self.checkpoint(session).await?;
        self.execute_tool_calls(session, &calls, scope, stream)
            .await
    }
}

impl Agent {
    pub(super) fn preferred_skill_names(&self, session: &Session) -> Vec<String> {
        let mut names = references(&super::mcp_tools::task_input(session));
        if let (Some(snapshot), Ok(execution)) = (&self.skill_snapshot, session.current_execution())
        {
            for reference in &execution.selected_skills {
                if let Ok(info) = snapshot.resolve(reference) {
                    if !names.contains(&info.name) {
                        names.push(info.name.clone());
                    }
                }
            }
        }
        names
    }
}
