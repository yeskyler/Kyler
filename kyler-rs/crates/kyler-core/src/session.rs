pub mod snapshot;
pub mod store;

use crate::{
    attachments::image::UserImage,
    context::{
        ContentItem, ContextMemory, Event, EventAuthor, ExecutionContext, ExecutionStatus, Role,
        TokenUsage, ToolResultStatus,
    },
    environment::SessionEnvironment,
    subagent::{ParentSessionRef, SubagentStatus, SubagentTask},
};

use std::collections::{HashMap, HashSet};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use kyler_protocol::api::{attachments::AttachmentInfo, permissions::ApprovalStatus};

pub use store::{SessionInfo, SessionStore};

pub const SESSION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub author: EventAuthor,
    pub schema_version: u32,
    pub session_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Optimistic persistence version; updated by SessionStore::save.
    pub revision: u64,
    pub events: Vec<Event>,
    pub memory: ContextMemory,
    pub state: HashMap<String, Value>,
    pub usage: TokenUsage,
    pub execution: Option<ExecutionContext>,
    pub executions: Vec<ExecutionContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<SessionEnvironment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ParentSessionRef>,
    #[serde(default)]
    pub subagent_usage: TokenUsage,
    #[serde(default)]
    pub subagent_tasks: HashMap<String, SubagentTask>,
}

impl Session {
    pub fn new() -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            author: EventAuthor::default(),
            schema_version: SESSION_SCHEMA_VERSION,
            session_id: Uuid::new_v4().to_string(),
            created_at: now,
            updated_at: now,
            revision: 0,
            events: Vec::new(),
            memory: ContextMemory::default(),
            state: HashMap::new(),
            usage: TokenUsage::default(),
            execution: None,
            executions: Vec::new(),
            environment: None,
            parent: None,
            subagent_usage: TokenUsage::default(),
            subagent_tasks: HashMap::new(),
        }
    }
    pub fn begin_execution(&mut self, input: &str) -> Result<()> {
        self.begin_execution_with_images(input, Vec::new())
    }
    pub fn begin_execution_with_images(
        &mut self,
        input: &str,
        images: Vec<UserImage>,
    ) -> Result<()> {
        self.begin_turn(input, images, Vec::new())
    }
    pub fn begin_turn(
        &mut self,
        input: &str,
        images: Vec<UserImage>,
        attachments: Vec<AttachmentInfo>,
    ) -> Result<()> {
        ensure!(
            attachments.len() <= kyler_protocol::api::attachments::MAX_ATTACHMENTS,
            "Too many attachments"
        );
        ensure!(
            !input.trim().is_empty() || !images.is_empty() || !attachments.is_empty(),
            "user input cannot be empty"
        );
        ensure!(
            images.len() <= kyler_protocol::api::images::MAX_IMAGES,
            "Too many images"
        );
        ensure!(
            !self.is_running(),
            "session already has a running execution"
        );
        if let Some(previous) = self.execution.take() {
            self.executions.push(previous);
        }
        let execution = ExecutionContext::new();
        self.memory.current_question_index = self.events.len();
        let event = Event::new(
            execution.execution_id.clone(),
            EventAuthor {
                name: "user".into(),
            },
            std::iter::once(ContentItem::Message {
                role: Role::User,
                content: input.to_string(),
            })
            .chain(images.into_iter().map(|image| ContentItem::Image { image }))
            .chain(
                attachments
                    .into_iter()
                    .map(|attachment| ContentItem::Attachment { attachment }),
            )
            .collect(),
        );
        self.execution = Some(execution);
        self.add_event(event);
        Ok(())
    }
    pub fn is_running(&self) -> bool {
        self.execution
            .as_ref()
            .is_some_and(|e| e.status == ExecutionStatus::Running)
    }
    pub fn current_execution(&self) -> Result<&ExecutionContext> {
        self.execution
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("session has no execution"))
    }
    pub fn current_execution_mut(&mut self) -> Result<&mut ExecutionContext> {
        self.execution
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("session has no execution"))
    }
    pub fn add_event(&mut self, event: Event) {
        self.events.push(event);
        self.touch();
    }
    pub fn touch(&mut self) {
        self.updated_at = chrono::Utc::now().timestamp_millis();
    }
    pub fn add_usage(&mut self, p: u32, c: u32, t: u32) {
        self.usage.add(p, c, t);
        if let Some(execution) = &mut self.execution {
            execution.usage.add(p, c, t);
        }
        self.touch();
    }
    /// Merge monotonically increasing usage once per task, even after repeated waits.
    pub fn record_subagent_tasks(&mut self, tasks: &[SubagentTask]) {
        for task in tasks {
            if self
                .subagent_tasks
                .get(&task.task_id)
                .is_some_and(|t| t.status.is_terminal())
            {
                continue;
            }
            let previous = self
                .subagent_tasks
                .get(&task.task_id)
                .map(|t| t.usage)
                .unwrap_or_default();
            let p = task
                .usage
                .prompt_tokens
                .saturating_sub(previous.prompt_tokens);
            let c = task
                .usage
                .completion_tokens
                .saturating_sub(previous.completion_tokens);
            let t = task
                .usage
                .total_tokens
                .saturating_sub(previous.total_tokens);
            self.subagent_usage.add(p, c, t);
            if let Some(execution) = self
                .execution
                .as_mut()
                .filter(|e| e.execution_id == task.parent_execution_id)
            {
                execution.subagent_usage.add(p, c, t);
            }
            self.subagent_tasks
                .insert(task.task_id.clone(), task.clone());
        }
        self.touch();
    }

    pub fn finish(
        &mut self,
        status: ExecutionStatus,
        output: Option<String>,
        error: Option<String>,
    ) -> Result<()> {
        self.repair_pending_calls("Execution interrupted; tool outcome is unknown. Do not automatically repeat side effects.");
        let execution = self.current_execution_mut()?;
        for approval in &mut execution.approvals {
            if approval.status == ApprovalStatus::Pending {
                approval.status = ApprovalStatus::Cancelled;
                approval.reason = "Execution ended; pending approvals are invalid.".into();
            }
        }
        execution.status = status;
        execution.final_result = output;
        execution.error = error;
        execution.finished_at = Some(chrono::Utc::now().timestamp_millis());
        self.touch();
        Ok(())
    }
    /// Caller must hold the per-session execution lease; never auto-replays tools.
    pub fn recover_interrupted(&mut self) -> Result<()> {
        if self.is_running() {
            for task in self
                .subagent_tasks
                .values_mut()
                .filter(|t| !t.status.is_terminal())
            {
                task.status = SubagentStatus::Cancelled;
                task.error=Some("Parent execution interrupted; in-process task not restored; external outcomes may be unknown".into());
                task.finished_at = Some(chrono::Utc::now().timestamp_millis());
            }
            self.finish(
                ExecutionStatus::Cancelled,
                None,
                Some("Previous execution was interrupted; tool outcomes may be unknown".into()),
            )?;
        }
        Ok(())
    }
    fn repair_pending_calls(&mut self, message: &str) {
        let mut pending = Vec::new();
        let mut completed = HashSet::new();
        for event in &self.events {
            for item in &event.content {
                match item {
                    ContentItem::ToolCall {
                        tool_call_id, name, ..
                    } => pending.push((
                        event.execution_id.clone(),
                        tool_call_id.clone(),
                        name.clone(),
                    )),
                    ContentItem::ToolResult { tool_call_id, .. } => {
                        completed.insert((event.execution_id.clone(), tool_call_id.clone()));
                    }
                    _ => {}
                }
            }
        }
        for (execution_id, id, name) in pending {
            if completed.insert((execution_id.clone(), id.clone())) {
                self.add_event(Event::new(
                    execution_id,
                    EventAuthor {
                        name: "runtime".into(),
                    },
                    vec![ContentItem::ToolResult {
                        tool_call_id: id,
                        name,
                        status: ToolResultStatus::Error,
                        model_content: None,
                        content: message.into(),
                    }],
                ));
            }
        }
    }
    /// Sorted and deduplicated so fixed questions retain their chronological position.
    pub fn visible_event_indices(&self) -> Vec<usize> {
        let mut indices: Vec<_> = (self.memory.window_start..self.events.len()).collect();
        indices.extend(self.memory.retained_questions.iter().copied());
        if !self.events.is_empty() {
            indices.push(self.memory.current_question_index);
        }
        indices.sort_unstable();
        indices.dedup();
        indices.retain(|&i| i < self.events.len());
        indices
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == SESSION_SCHEMA_VERSION,
            "unsupported session schema version"
        );
        Uuid::parse_str(&self.session_id)?;
        ensure!(
            self.memory.window_start <= self.events.len(),
            "invalid history window"
        );
        if !self.events.is_empty() {
            ensure!(
                self.memory.current_question_index < self.events.len(),
                "invalid current question index"
            );
            for &i in self
                .memory
                .retained_questions
                .iter()
                .chain(std::iter::once(&self.memory.current_question_index))
            {
                ensure!(
                    self.events.get(i).is_some_and(|e| !e.content.is_empty()
                        && e.content.len()
                            <= kyler_protocol::api::attachments::MAX_ATTACHMENTS
                                + kyler_protocol::api::images::MAX_IMAGES
                                + 1
                        && e.content.iter().skip(1).all(|item| matches!(
                            item,
                            ContentItem::Image { .. } | ContentItem::Attachment { .. }
                        ))
                        && matches!(
                            e.content[0],
                            ContentItem::Message {
                                role: Role::User,
                                ..
                            }
                        )),
                    "invalid retained user question"
                );
            }
        }
        Ok(())
    }
}
impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Session;
    use crate::context::ExecutionStatus;

    use serde_json::Value;

    #[test]
    fn legacy_session_without_subagent_fields_remains_loadable() {
        let mut session = Session::new();
        session.begin_execution("legacy question").unwrap();
        session.add_usage(10, 2, 12);
        session
            .finish(ExecutionStatus::Completed, Some("answer".into()), None)
            .unwrap();
        let mut old = serde_json::to_value(session).unwrap();
        for key in ["parent", "subagent_usage", "subagent_tasks"] {
            old.as_object_mut().unwrap().remove(key);
        }
        old["execution"]
            .as_object_mut()
            .unwrap()
            .remove("subagent_usage");
        let restored: Session = serde_json::from_value(old).unwrap();
        restored.validate().unwrap();
        assert!(restored.parent.is_none());
        assert!(restored.subagent_tasks.is_empty());
        assert_eq!(restored.subagent_usage.total_tokens, 0);
        assert_eq!(restored.usage.total_tokens, 12);
    }

    #[test]
    fn new_turn_resets_execution_but_retains_session_context() {
        let mut s = Session::new();
        s.begin_execution("first").unwrap();
        let first = s.current_execution().unwrap().execution_id.clone();
        s.state.insert("shared".into(), Value::Bool(true));
        s.current_execution_mut().unwrap().current_step = 4;
        s.finish(ExecutionStatus::Completed, Some("answer".into()), None)
            .unwrap();
        s.begin_execution("second").unwrap();
        assert_ne!(first, s.current_execution().unwrap().execution_id);
        assert_eq!(s.current_execution().unwrap().current_step, 0);
        assert!(s.current_execution().unwrap().final_result.is_none());
        assert_eq!(s.events.len(), 2);
        assert_eq!(s.executions.len(), 1);
        assert_eq!(s.memory.current_question_index, 1);
        assert_eq!(s.state["shared"], true);
        assert!(s.begin_execution("third").is_err());
    }
}
