//! Durable execution events and session checkpoints.
use super::Agent;
use crate::{
    context::{ContentItem, Event, ToolResultStatus},
    session::Session,
};

use anyhow::Result;
use async_openai::types::chat::ChatCompletionMessageToolCalls;

use kyler_protocol::api::history::ToolPresentation;

impl Agent {
    pub(super) async fn checkpoint(&self, session: &mut Session) -> Result<()> {
        if let Some(store) = &self.session_store {
            store.save(session).await?;
        }
        Ok(())
    }
    pub(super) fn record(&self, session: &mut Session, content: Vec<ContentItem>) -> Result<()> {
        let id = session.current_execution()?.execution_id.clone();
        session.add_event(Event::new(id, session.author.clone(), content));
        Ok(())
    }
    pub(super) fn record_tool_calls(
        &self,
        session: &mut Session,
        calls: &[ChatCompletionMessageToolCalls],
    ) -> Result<()> {
        for call in calls {
            if let ChatCompletionMessageToolCalls::Function(call) = call {
                if let Some(tool) = self.toolbox.get(&call.function.name) {
                    let arguments =
                        serde_json::from_str(&call.function.arguments).unwrap_or_default();
                    let execution = session.current_execution_mut()?;
                    execution.tool_presentations.insert(
                        call.id.clone(),
                        ToolPresentation {
                            step: execution.current_step,
                            activity: tool.activity_kind(),
                            source: tool.source_for_arguments(&arguments),
                            started_at: None,
                            finished_at: None,
                        },
                    );
                }
            }
        }
        let content = calls
            .iter()
            .filter_map(|call| match call {
                ChatCompletionMessageToolCalls::Function(f) => Some(ContentItem::ToolCall {
                    tool_call_id: f.id.clone(),
                    name: f.function.name.clone(),
                    arguments: serde_json::from_str(&f.function.arguments)
                        .unwrap_or(serde_json::Value::Null),
                }),
                _ => None,
            })
            .collect();
        self.record(session, content)
    }
    pub(super) fn record_tool_result(
        &self,
        session: &mut Session,
        id: &str,
        name: &str,
        status: ToolResultStatus,
        content: String,
    ) -> Result<()> {
        if let Some(metadata) = session
            .current_execution_mut()?
            .tool_presentations
            .get_mut(id)
        {
            metadata.finished_at = Some(chrono::Utc::now().timestamp_millis());
        }
        let model_content = self.compress_tool_output(name, status, &content);
        self.record(
            session,
            vec![ContentItem::ToolResult {
                tool_call_id: id.into(),
                name: name.into(),
                status,
                content,
                model_content,
            }],
        )
    }
}
