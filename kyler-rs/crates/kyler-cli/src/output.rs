use crate::{CliError, Result};

use std::io::{self, Write};

use serde::Serialize;

use kyler_client::ClientEvent;
use kyler_protocol::api::{event::EventPayload, permissions::ApprovalStatus, run::RunInfo};

pub fn json(value: &impl Serialize) -> Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, value).map_err(|e| {
        CliError::new(
            if e.io_error_kind() == Some(io::ErrorKind::BrokenPipe) {
                0
            } else {
                3
            },
            e.to_string(),
        )
    })?;
    writeln!(out).map_err(CliError::io)
}
pub fn text(value: &str) -> Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(value.as_bytes())
        .and_then(|_| out.flush())
        .map_err(CliError::io)
}
/// Tracks only the current model step so the terminal snapshot does not duplicate streamed text.
pub struct Output {
    json: bool,
    final_only: bool,
    step: Option<u32>,
    current: String,
    line_open: bool,
    gap: bool,
}
impl Output {
    pub fn new(json: bool, final_only: bool, resumed: bool) -> Self {
        Self {
            json,
            final_only,
            step: None,
            current: String::new(),
            line_open: false,
            gap: resumed,
        }
    }
    pub fn newline(&mut self) -> Result<()> {
        if self.line_open {
            text("\n")?;
            self.line_open = false;
        }
        Ok(())
    }
    pub fn event(&mut self, event: &ClientEvent) -> Result<()> {
        if self.json {
            return match event {
                ClientEvent::Server(event) => json(event),
                ClientEvent::Reconnecting {
                    attempt,
                    last_event_id,
                } => json(
                    &serde_json::json!({"type":"client_reconnecting","attempt":attempt,"last_event_id":last_event_id}),
                ),
                ClientEvent::Gap { message } => {
                    json(&serde_json::json!({"type":"client_gap","message":message}))
                }
                ClientEvent::Result(run) => {
                    json(&serde_json::json!({"type":"client_result","run":run}))
                }
            };
        }
        match event {
            ClientEvent::Server(event) => match &event.payload {
                EventPayload::StepStarted { step } => {
                    if self.step != Some(*step) {
                        self.newline()?;
                        self.current.clear();
                        self.step = Some(*step);
                    }
                }
                EventPayload::TextDelta { step, text: delta } if !self.final_only && !self.gap => {
                    if self.step != Some(*step) {
                        self.newline()?;
                        self.current.clear();
                        self.step = Some(*step);
                    }
                    text(delta)?;
                    self.current.push_str(delta);
                    self.line_open = !delta.ends_with('\n');
                }
                EventPayload::ApprovalUpdated { approval }
                    if approval.status == ApprovalStatus::Pending =>
                {
                    self.newline()?;
                    eprintln!(
                        "Approval required for {}. Review session {} in the TUI.",
                        approval.tool, approval.session_id
                    );
                }
                EventPayload::ToolStarted { name, .. } if !self.final_only => {
                    self.newline()?;
                    eprintln!("Tool: {name}");
                }
                EventPayload::ToolFinished { name, status, .. } if !self.final_only => {
                    eprintln!("Tool: {name} ({status})")
                }
                _ => {}
            },
            ClientEvent::Reconnecting {
                attempt,
                last_event_id,
            } => eprintln!("Reconnecting ({attempt}), after event {last_event_id}…"),
            ClientEvent::Gap { message } => {
                self.newline()?;
                self.gap = true;
                eprintln!("{message}");
            }
            ClientEvent::Result(run) => self.finish(run)?,
        }
        Ok(())
    }
    fn finish(&mut self, run: &RunInfo) -> Result<()> {
        if let Some(answer) = &run.output {
            if self.final_only {
                text(answer)?;
                self.line_open = !answer.ends_with('\n');
            } else if !self.gap && answer.starts_with(&self.current) {
                let tail = &answer[self.current.len()..];
                if !tail.is_empty() {
                    text(tail)?;
                    self.line_open = !tail.ends_with('\n');
                }
            } else {
                self.newline()?;
                eprintln!("Final answer:");
                text(answer)?;
                self.line_open = !answer.ends_with('\n');
            }
        }
        self.newline()
    }
}
