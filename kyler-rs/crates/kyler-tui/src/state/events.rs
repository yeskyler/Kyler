use super::{Entry, Phase, State};
use crate::components::messages::assistant::AgentMessageCell;

use std::time::Instant;

use kyler_client::ClientEvent;
use kyler_protocol::api::{event::EventPayload, permissions::ApprovalStatus, run::RunStatus};

impl State {
    pub fn stream(&mut self, event: ClientEvent) {
        self.transcript_version += 1;
        self.dirty = true;
        match event {
            ClientEvent::Server(event) => {
                if self.run.as_deref() != Some(&event.run_id) || event.seq <= self.cursor {
                    return;
                }
                self.cursor = event.seq;
                match event.payload {
                    EventPayload::QuestionUpdated { question } => {
                        self.finish_assistant();
                        self.questions.update(question);
                        crate::questions::history(self);
                    }
                    EventPayload::ApprovalUpdated { approval } => {
                        if approval.status == ApprovalStatus::Pending {
                            self.finish_assistant();
                        }
                        self.update_approval(approval);
                    }
                    EventPayload::PermissionsBound { policy } => {
                        self.active_permissions = Some(policy)
                    }
                    EventPayload::RunStarted { model, .. } => self.active_model = model,
                    EventPayload::StepStarted { step } => {
                        if self.step != Some(step) {
                            self.finish_assistant();
                            self.step = Some(step);
                        }
                    }
                    EventPayload::ReasoningDelta { step, text }
                        if !self.gap && !text.is_empty() =>
                    {
                        self.finish_assistant();
                        if self.step == Some(step)
                            && let Some(Entry::Message { role, content }) = self.entries.last_mut()
                            && role == "thinking"
                        {
                            content.push_str(&text);
                        } else {
                            self.entries.push(Entry::Message {
                                role: "thinking".into(),
                                content: text,
                            });
                        }
                        self.step = Some(step);
                    }
                    EventPayload::TextDelta { step, text } if !self.gap => {
                        if self.step != Some(step) {
                            self.finish_assistant();
                            self.step = Some(step);
                        }
                        if let Some(index) = self.assistant {
                            if let Entry::Assistant(cell) = &mut self.entries[index] {
                                cell.push(&text);
                            }
                        } else {
                            self.assistant = Some(self.entries.len());
                            let mut cell = AgentMessageCell::default();
                            cell.push(&text);
                            self.entries.push(Entry::Assistant(cell));
                        }
                    }
                    EventPayload::ToolStarted {
                        call_id,
                        name,
                        activity,
                        source,
                        summary,
                    } => {
                        self.finish_assistant();
                        self.entries.push(Entry::Tool {
                            id: call_id,
                            name,
                            status: "running".into(),
                            started: Instant::now(),
                            seconds: None,
                            activity: Some(activity),
                            source,
                            output_bytes: None,
                            summary,
                            error: None,
                        });
                    }
                    EventPayload::ToolFinished {
                        call_id,
                        name,
                        status,
                        output_bytes: bytes,
                        error,
                        ..
                    } => {
                        if let Some(Entry::Tool {
                            status: state,
                            started,
                            seconds,
                            output_bytes,
                            error: tool_error,
                            ..
                        }) = self
                            .entries
                            .iter_mut()
                            .rev()
                            .find(|e| matches!(e,Entry::Tool {id,..} if id==&call_id))
                        {
                            *state = status;
                            *tool_error = error;
                            *output_bytes = Some(bytes);
                            *seconds = Some(started.elapsed().as_secs_f64());
                        } else {
                            self.entries.push(Entry::Tool {
                                id: call_id,
                                name,
                                status,
                                started: Instant::now(),
                                seconds: Some(0.0),
                                activity: Default::default(),
                                source: Default::default(),
                                output_bytes: Some(bytes),
                                summary: None,
                                error,
                            });
                        }
                    }
                    _ => {}
                }
            }
            ClientEvent::Reconnecting { attempt, .. } => {
                self.notice(format!("Disconnected; reconnecting (attempt {attempt})…"))
            }
            ClientEvent::Gap { .. } => {
                self.gap = true;
                self.notice("Some streamed content has expired; waiting for the complete result.");
            }
            ClientEvent::Result(result) => {
                if self.run.as_deref() != Some(&result.run_id) || self.phase == Phase::Ready {
                    return;
                }
                let seconds = result
                    .finished_at
                    .map(|finished| finished.saturating_sub(result.created_at).max(0) as u64 / 1000)
                    .unwrap_or_else(|| {
                        self.run_started
                            .map(|start| start.elapsed().as_secs())
                            .unwrap_or(0)
                    });
                if let Some(answer) = result.output {
                    if self.gap {
                        self.notice("Complete answer recovered from the final snapshot.");
                    }
                    let after_activity = self
                        .entries
                        .iter()
                        .rev()
                        .take_while(
                            |entry| !matches!(entry, Entry::Message { role, .. } if role == "user"),
                        )
                        .any(|entry| matches!(entry, Entry::Tool { .. }));
                    if let Some(index) = self.assistant {
                        if let Entry::Assistant(cell) = &mut self.entries[index] {
                            cell.finish(Some(answer));
                            cell.final_separator = after_activity;
                        }
                    } else {
                        let mut cell = AgentMessageCell::complete(answer);
                        cell.final_separator = after_activity;
                        self.entries.push(Entry::Assistant(cell));
                    }
                }
                self.finish_assistant();
                let model_failed = result
                    .error
                    .as_ref()
                    .is_some_and(|error| error.code.starts_with("model_"));
                if let Some(error) = result.error {
                    self.entries.push(Entry::Error {
                        title: if model_failed {
                            "Model error"
                        } else {
                            "Run error"
                        }
                        .into(),
                        message: error.message,
                    });
                    if model_failed {
                        self.failed_submission = self.pending.take();
                        if self.failed_submission.is_some() && !self.restore_failed_submission() {
                            self.notice("Failed input retained. Clear the current draft, then press Ctrl+R to restore it.");
                        }
                    }
                }
                if result.status == RunStatus::Cancelled {
                    self.notice("Task cancelled.");
                }
                self.entries.push(Entry::RunTiming {
                    label: match result.status {
                        RunStatus::Cancelled => "Cancelled after",
                        RunStatus::Failed => "Failed after",
                        _ => "Worked for",
                    }
                    .into(),
                    seconds,
                });
                self.run_started = None;
                self.phase = Phase::Ready;
                self.pending = None;
                self.cancel_requested = false;
                self.cancel_deadline = None;
                self.assistant = None;
            }
        }
    }
    /// The UI clock, not network packet boundaries, controls visible text.
    pub fn advance_messages(&mut self) {
        let changed = self
            .assistant
            .and_then(|index| self.entries.get_mut(index))
            .is_some_and(|entry| {
                if let Entry::Assistant(cell) = entry {
                    !cell.is_finished() && cell.advance()
                } else {
                    false
                }
            });
        if changed {
            self.transcript_version += 1;
            self.dirty = true;
        }
    }
    fn finish_assistant(&mut self) {
        if let Some(index) = self.assistant.take() {
            if let Entry::Assistant(cell) = &mut self.entries[index] {
                cell.finish(None);
            }
        }
    }
}
