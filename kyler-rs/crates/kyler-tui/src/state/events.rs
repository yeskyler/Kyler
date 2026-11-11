use super::{Entry, Phase, State};
use crate::agent::AgentEvent;
use crate::components::messages::assistant::{AgentMessageCell, ReasoningCell};

use std::time::Instant;

use kyler_protocol::api::{event::EventPayload, permissions::ApprovalStatus, run::RunStatus};

impl State {
    pub fn stream(&mut self, event: AgentEvent) {
        self.transcript_version += 1;
        self.dirty = true;
        match event {
            AgentEvent::Event(event) => {
                if self.run.as_deref() != Some(&event.run_id) || event.seq <= self.cursor {
                    return;
                }
                self.cursor = event.seq;
                let queued = matches!(&event.payload, EventPayload::ToolQueued { .. });
                match event.payload {
                    EventPayload::ContextCompaction { compaction } => {
                        self.finish_reasoning();
                        self.finish_assistant();
                        self.compaction(compaction);
                    }

                    EventPayload::QuestionUpdated { question } => {
                        self.finish_reasoning();
                        self.finish_assistant();
                        self.questions.update(question);
                        crate::questions::history(self);
                    }
                    EventPayload::ApprovalUpdated { approval } => {
                        if approval.status == ApprovalStatus::Pending {
                            self.finish_reasoning();
                            self.finish_assistant();
                        }
                        if let Some(Entry::Tool { status, .. }) = self.entries.iter_mut().rev().find(|entry| matches!(entry, Entry::Tool { id, .. } if id == &approval.tool_call_id)) {
                            if approval.status == ApprovalStatus::Pending { *status = "pending".into(); }
                        }
                        self.update_approval(approval);
                    }
                    EventPayload::PermissionsBound { policy } => {
                        self.active_permissions = Some(policy)
                    }
                    EventPayload::RunStarted { model, .. } => self.active_model = model,
                    EventPayload::StepStarted { step } => {
                        if self.step != Some(step) {
                            self.finish_reasoning();
                            self.finish_assistant();
                            self.step = Some(step);
                        }
                    }
                    EventPayload::ReasoningDelta { step, text } if !text.is_empty() => {
                        self.finish_assistant();
                        if self.step != Some(step) {
                            self.finish_reasoning();
                        }
                        if let Some(Entry::Reasoning(cell)) =
                            self.reasoning.and_then(|index| self.entries.get_mut(index))
                        {
                            cell.content.push_str(&text);
                        } else {
                            self.reasoning = Some(self.entries.len());
                            self.entries
                                .push(Entry::Reasoning(ReasoningCell::streaming(text)));
                        }
                        self.step = Some(step);
                    }
                    EventPayload::TextDelta { step, text } => {
                        self.finish_reasoning();
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
                    EventPayload::ToolCallDelta { call_id, name } => {
                        self.finish_reasoning();
                        self.finish_assistant();
                        if let Some(Entry::Tool { name: current, .. }) =
                            self.entries.iter_mut().rev().find(
                                |entry| matches!(entry, Entry::Tool { id, .. } if id == &call_id),
                            )
                        {
                            *current = name;
                        } else {
                            self.entries.push(Entry::Tool {
                                id: call_id,
                                name,
                                status: "preparing".into(),
                                started: Instant::now(),
                                seconds: None,
                                activity: None,
                                source: Default::default(),
                                output_bytes: None,
                                error: None,
                                summary: None,
                            });
                        }
                    }
                    EventPayload::ToolQueued {
                        call_id,
                        name,
                        activity,
                        source,
                        summary,
                    }
                    | EventPayload::ToolStarted {
                        call_id,
                        name,
                        activity,
                        source,
                        summary,
                    } => {
                        self.finish_reasoning();
                        self.finish_assistant();
                        if let Some(Entry::Tool {
                            status,
                            started,
                            name: current_name,
                            activity: current_activity,
                            source: current_source,
                            summary: current_summary,
                            ..
                        }) =
                            self.entries.iter_mut().rev().find(
                                |entry| matches!(entry, Entry::Tool { id, .. } if id == &call_id),
                            )
                        {
                            *current_name = name;
                            *current_activity = Some(activity);
                            *current_source = source;
                            *current_summary = summary;
                            if queued && status == "preparing" {
                                *status = "queued".into();
                            }
                            if !queued {
                                *status = "running".into();
                                *started = Instant::now();
                            }
                            return;
                        }
                        let waiting = queued && self.entries.iter().any(|entry| matches!(entry, Entry::Approval { record, .. } if record.tool_call_id == call_id && record.status == ApprovalStatus::Pending));
                        self.entries.push(Entry::Tool {
                            id: call_id,
                            name,
                            status: if waiting {
                                "pending"
                            } else if queued {
                                "queued"
                            } else {
                                "running"
                            }
                            .into(),
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
                            let executed = state == "running";
                            *state = status;
                            *tool_error = error;
                            *output_bytes = Some(bytes);
                            *seconds = executed.then(|| started.elapsed().as_secs_f64());
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
            AgentEvent::Result(result) => {
                if self.run.as_deref() != Some(&result.run_id) || self.phase == Phase::Ready {
                    return;
                }
                self.finish_reasoning();
                self.settle_compactions(result.status == RunStatus::Cancelled);
                let seconds = result
                    .finished_at
                    .map(|finished| finished.saturating_sub(result.created_at).max(0) as u64 / 1000)
                    .unwrap_or_else(|| {
                        self.run_started
                            .map(|start| start.elapsed().as_secs())
                            .unwrap_or(0)
                    });
                if let Some(answer) = result.output {
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
                for entry in self
                    .entries
                    .iter_mut()
                    .rev()
                    .take_while(|entry| !matches!(entry, Entry::RunTiming { .. }))
                {
                    if let Entry::Tool { status, .. } = entry {
                        if matches!(
                            status.as_str(),
                            "preparing" | "running" | "queued" | "pending"
                        ) {
                            *status = if result.status == RunStatus::Cancelled {
                                "cancelled"
                            } else {
                                "unknown"
                            }
                            .into();
                        }
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
