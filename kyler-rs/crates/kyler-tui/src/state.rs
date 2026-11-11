mod events;

use crate::{
    attachments::LocalAttachment,
    components::{
        composer,
        messages::assistant::{AgentMessageCell, ReasoningCell},
        model_config::ModelConfigForm,
    },
};

use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

use ratatui_textarea::TextArea;

use kyler_protocol::{
    activity::{ActivityKind, ToolSource},
    api::{
        ServerInfo, SessionUsage,
        history::{HistoryContent, HistoryEvent, HistoryExecution},
        mcp::McpServerInfo,
        models::{ModelCatalog, ModelInfo, SessionModel},
        permissions::{ApprovalRecord, ApprovalStatus, PermissionSnapshot, PermissionState},
        skills::{SkillDetails, SkillInfo},
        tools::ToolDetails,
    },
};

#[derive(Debug, Clone)]
pub enum Entry {
    Compaction(kyler_protocol::api::compaction::CompactionInfo),
    Assistant(AgentMessageCell),
    Reasoning(ReasoningCell),
    RunTiming {
        label: String,
        seconds: u64,
    },
    Error {
        title: String,
        message: String,
    },
    Approval {
        record: Box<ApprovalRecord>,
        expanded: bool,
        choice: usize,
    },
    Message {
        role: String,
        content: String,
    },
    Tool {
        id: String,
        name: String,
        status: String,
        started: Instant,
        seconds: Option<f64>,
        activity: Option<ActivityKind>,
        source: ToolSource,
        output_bytes: Option<usize>,
        summary: Option<String>,
        error: Option<String>,
    },
    Notice(String),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Loading,
    Ready,
    Submitting,
    Running,
    Cancelling,
    Detached,
}
#[derive(Clone)]
pub struct Submission {
    pub resume: bool,
    pub attachments: Vec<LocalAttachment>,
    pub input: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Model,
    Permissions,
    Mcp,
    Skills,
    Help,
    Approval,
}
pub struct State {
    pub questions: crate::questions::QuestionUi,
    pub tool_details: Option<ToolDetails>,
    pub tool_scroll: usize,
    pub at_selected: usize,
    pub skill_selected: usize,
    pub skill_dismissed: Option<String>,
    pub panel_parent: Option<(String, usize)>,
    pub model_form: Option<ModelConfigForm>,
    pub model_details: bool,
    pub model_applying: bool,
    pub model_scroll: usize,
    pub permissions_details: bool,
    pub help_scroll: usize,
    pub approval_details: Option<String>,
    pub approval_scroll: usize,

    pub permissions: Option<PermissionState>,
    pub active_permissions: Option<PermissionSnapshot>,
    pub permissions_open: bool,
    pub permissions_selected: usize,
    pub permissions_scroll: u16,
    pub permissions_confirm: bool,
    pub permissions_busy: bool,
    pub permissions_error: Option<String>,
    pub permissions_epoch: u64,
    pub approval_focus: bool,
    pub approval_busy: bool,
    pub approval_error: Option<String>,

    pub active_model: Option<ModelInfo>,
    pub model_catalog: Option<ModelCatalog>,
    pub session_model: Option<SessionModel>,
    pub model_open: bool,
    pub model_selected: usize,
    pub model_busy: bool,
    pub model_error: Option<String>,
    pub skills: Vec<SkillInfo>,
    pub skills_open: bool,
    pub skills_selected: usize,
    pub skill_details: Option<SkillDetails>,
    pub skills_scroll: usize,
    pub skills_busy: bool,
    pub skills_error: Option<String>,
    pub slash_selected: usize,
    pub slash_dismissed: Option<String>,
    pub mcp: Vec<McpServerInfo>,
    pub mcp_open: bool,
    pub mcp_selected: usize,
    pub mcp_details: bool,
    pub mcp_scroll: usize,
    pub mcp_error: Option<String>,
    pub generation: u64,
    pub transcript_version: u64,
    pub session: Option<String>,
    pub info: Option<ServerInfo>,
    pub usage: Option<SessionUsage>,
    pub entries: Vec<Entry>,
    pub attachments: Vec<LocalAttachment>,
    pub image_busy: bool,
    pub draft_epoch: u64,
    pub composer: TextArea<'static>,
    pub phase: Phase,
    pub run: Option<String>,
    pub run_started: Option<Instant>,
    pub cursor: u64,
    pub pending: Option<Submission>,
    pub failed_submission: Option<Submission>,
    pub cancel_requested: bool,
    pub cancel_deadline: Option<Instant>,
    pub assistant: Option<usize>,
    pub reasoning: Option<usize>,
    pub step: Option<u32>,
    pub follow: bool,
    pub scroll: usize,
    pub max_scroll: usize,
    pub help: bool,
    pub confirm_exit: bool,
    pub dirty: bool,
}
impl State {
    pub fn new(session: Option<String>) -> Self {
        Self {
            at_selected: 0,
            skill_selected: 0,
            skill_dismissed: None,
            questions: Default::default(),
            tool_details: None,
            tool_scroll: 0,
            panel_parent: None,
            model_form: None,
            model_details: false,
            model_applying: false,
            model_scroll: 0,
            permissions_details: false,
            help_scroll: 0,
            approval_details: None,
            approval_scroll: 0,
            permissions: None,
            active_permissions: None,
            permissions_open: false,
            permissions_selected: 0,
            permissions_scroll: 0,
            permissions_confirm: false,
            permissions_busy: false,
            permissions_error: None,
            permissions_epoch: 0,
            approval_focus: false,
            approval_busy: false,
            approval_error: None,
            active_model: None,
            model_catalog: None,
            session_model: None,
            model_open: false,
            model_selected: 0,
            model_busy: false,
            model_error: None,
            skills: Vec::new(),
            skills_open: false,
            skills_selected: 0,
            skill_details: None,
            skills_scroll: 0,
            skills_busy: false,
            skills_error: None,
            slash_selected: 0,
            slash_dismissed: None,
            mcp: Vec::new(),
            mcp_open: false,
            mcp_selected: 0,
            mcp_details: false,
            mcp_scroll: 0,
            mcp_error: None,
            generation: 0,
            transcript_version: 0,
            session,
            info: None,
            usage: None,
            entries: Vec::new(),
            attachments: Vec::new(),
            image_busy: false,
            draft_epoch: 0,
            composer: composer::new(),
            phase: Phase::Loading,
            run: None,
            run_started: None,
            cursor: 0,
            pending: None,
            failed_submission: None,
            cancel_requested: false,
            cancel_deadline: None,
            assistant: None,
            reasoning: None,
            step: None,
            follow: true,
            scroll: 0,
            max_scroll: 0,
            help: false,
            confirm_exit: false,
            dirty: true,
        }
    }
    pub fn open_panel(&mut self, panel: Panel) {
        self.tool_details = None;
        self.help = false;
        self.model_open = false;
        self.permissions_open = false;
        self.mcp_open = false;
        self.skills_open = false;
        self.approval_details = None;
        self.panel_parent = None;
        self.model_form = None;
        self.model_details = false;
        self.model_scroll = 0;
        self.permissions_details = false;
        self.permissions_confirm = false;
        self.mcp_details = false;
        self.mcp_scroll = 0;
        self.skill_details = None;
        self.skills_scroll = 0;
        self.help_scroll = 0;
        match panel {
            Panel::Model => self.model_open = true,
            Panel::Permissions => self.permissions_open = true,
            Panel::Mcp => self.mcp_open = true,
            Panel::Skills => self.skills_open = true,
            Panel::Help => self.help = true,
            Panel::Approval => {}
        }
        self.dirty = true;
    }
    pub fn panel_open(&self) -> bool {
        self.questions.pending().is_some()
            || self.tool_details.is_some()
            || self.help
            || self.model_open
            || self.permissions_open
            || self.mcp_open
            || self.skills_open
            || self.approval_details.is_some()
            || self.confirm_exit
    }
    /// Return to the command query only when this surface was opened from that query.
    pub fn back_to_commands(&mut self) {
        if self.help
            || self.model_open
            || self.permissions_open
            || self.mcp_open
            || self.skills_open
            || self.approval_details.is_some()
        {
            return;
        }
        if let Some((query, selected)) = self.panel_parent.take() {
            self.composer = composer::new();
            self.composer.insert_str(query);
            self.slash_selected = selected;
            self.slash_dismissed = None;
        }
    }
    pub fn pending_approval(&self) -> Option<usize> {
        self.entries.iter().position(|e| matches!(e, Entry::Approval { record, .. } if record.status == kyler_protocol::api::permissions::ApprovalStatus::Pending))
    }
    pub fn update_approval(&mut self, record: ApprovalRecord) {
        if record.status == ApprovalStatus::Pending {}
        if !record.requires_user && record.status != ApprovalStatus::Denied {
            return;
        }
        if let Some(Entry::Approval {
            record: existing, ..
        }) = self
            .entries
            .iter_mut()
            .find(|e| matches!(e, Entry::Approval { record: old, .. } if old.id == record.id))
        {
            if existing.status != ApprovalStatus::Pending
                && record.status == ApprovalStatus::Pending
            {
                return;
            }
            if existing.status == record.status && existing.reason == record.reason {
                return;
            }
            *existing = Box::new(record);
        } else {
            if record.status == ApprovalStatus::Pending {
                self.approval_focus =
                    self.composer.lines().iter().all(|line| line.is_empty()) && !self.panel_open();
                self.follow = true;
            }
            self.entries.push(Entry::Approval {
                record: Box::new(record),
                expanded: false,
                choice: 2,
            });
        }
        if self.pending_approval().is_none() {
            self.approval_focus = false;
            self.approval_details = None;
            self.approval_error = None;
        }
        self.transcript_version += 1;
        self.dirty = true;
    }
    pub fn finish_reasoning(&mut self) {
        if let Some(index) = self.reasoning.take() {
            if let Some(Entry::Reasoning(cell)) = self.entries.get_mut(index) {
                cell.finish();
                self.transcript_version += 1;
                self.dirty = true;
            }
        }
    }
    pub fn toggle_reasoning(&mut self, index: usize) {
        if let Some(Entry::Reasoning(cell)) = self.entries.get_mut(index) {
            cell.toggle();
            if cell.open() {
                self.follow = false;
            }
            self.transcript_version += 1;
            self.dirty = true;
        }
    }
    pub fn toggle_latest_reasoning(&mut self) {
        if let Some(index) = self
            .entries
            .iter()
            .rposition(|entry| matches!(entry, Entry::Reasoning(_)))
        {
            self.toggle_reasoning(index);
        }
    }
    pub fn displayed_model(&self) -> &str {
        if matches!(
            self.phase,
            Phase::Running | Phase::Cancelling | Phase::Submitting
        ) {
            if let Some(model) = &self.active_model {
                return &model.model;
            }
        }
        if self.session_model.as_ref().is_some_and(|s| {
            s.selected.is_none()
                && (!matches!(
                    self.phase,
                    Phase::Running | Phase::Cancelling | Phase::Submitting
                ) || s.active.is_none())
        }) {
            return "Model unavailable";
        }
        let model = self.session_model.as_ref().and_then(|s| {
            if matches!(
                self.phase,
                Phase::Running | Phase::Cancelling | Phase::Submitting
            ) {
                s.active.as_ref().or(s.selected.as_ref())
            } else {
                s.selected.as_ref()
            }
        });
        model
            .map(|m| m.model.as_str())
            .or_else(|| self.info.as_ref().map(|i| i.model.as_str()))
            .unwrap_or("Connecting…")
    }
    pub(super) fn compaction(
        &mut self,
        compaction: kyler_protocol::api::compaction::CompactionInfo,
    ) {
        if let Some(Entry::Compaction(previous)) = self
            .entries
            .iter_mut()
            .find(|entry| matches!(entry, Entry::Compaction(info) if info.id == compaction.id))
        {
            *previous = compaction;
        } else {
            self.entries.push(Entry::Compaction(compaction));
        }
    }
    pub(super) fn settle_compactions(&mut self, cancelled: bool) {
        use kyler_protocol::api::compaction::CompactionStatus;
        for entry in &mut self.entries {
            if let Entry::Compaction(info) = entry {
                if info.status == CompactionStatus::Running {
                    info.status = if cancelled {
                        CompactionStatus::Cancelled
                    } else {
                        CompactionStatus::Interrupted
                    };
                }
            }
        }
    }
    pub fn notice(&mut self, text: impl Into<String>) {
        self.transcript_version += 1;
        self.entries.push(Entry::Notice(text.into()));
        self.dirty = true;
    }
    pub fn history(&mut self, events: Vec<HistoryEvent>, executions: Vec<HistoryExecution>) {
        self.questions.reset_history();
        self.entries.clear();
        self.reasoning = None;
        let mut calls = HashMap::new();
        let mut last_assistant = HashMap::new();
        let mut with_tools = HashSet::new();
        let mut cancelled_calls = HashSet::new();
        let executions = executions
            .into_iter()
            .map(|execution| (execution.execution_id.clone(), execution))
            .collect::<HashMap<_, _>>();
        let mut previous: Option<String> = None;
        for event in events {
            if previous
                .as_ref()
                .is_some_and(|id| id != &event.execution_id)
            {
                if let Some(execution) = previous.as_ref().and_then(|id| executions.get(id)) {
                    self.history_timing(execution);
                }
            }
            previous = Some(event.execution_id.clone());
            let user = event
                .content
                .iter()
                .filter_map(|item| match item {
                    HistoryContent::Message { role, text } if role == "user" => Some(text.clone()),
                    HistoryContent::Attachment { attachment } => {
                        Some(format!("[Attachment: {}]", attachment.name))
                    }
                    HistoryContent::Image { name } => Some(format!("[Image: {name}]")),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if !user.is_empty() {
                self.entries.push(Entry::Message {
                    role: "user".into(),
                    content: user.join("\n"),
                });
            }
            for item in event.content {
                match item {
                    HistoryContent::ContextCompaction { compaction } => self.compaction(compaction),
                    HistoryContent::Reasoning { text } if !text.trim().is_empty() => {
                        self.entries
                            .push(Entry::Reasoning(ReasoningCell::complete(text)));
                    }
                    HistoryContent::Message { role, text } if role == "assistant" => {
                        last_assistant.insert(event.execution_id.clone(), self.entries.len());
                        self.entries
                            .push(Entry::Assistant(AgentMessageCell::complete(text)));
                    }
                    HistoryContent::ToolCall {
                        call_id,
                        name,
                        summary,
                        presentation,
                        ..
                    } => {
                        with_tools.insert(event.execution_id.clone());
                        if executions
                            .get(&event.execution_id)
                            .is_some_and(|execution| execution.status == "cancelled")
                            && presentation
                                .as_ref()
                                .is_some_and(|metadata| metadata.finished_at.is_none())
                        {
                            cancelled_calls.insert((event.execution_id.clone(), call_id.clone()));
                        }
                        calls.insert(
                            (event.execution_id.clone(), call_id.clone()),
                            self.entries.len(),
                        );
                        self.entries.push(Entry::Tool {
                            id: call_id,
                            name,
                            status: if executions
                                .get(&event.execution_id)
                                .is_some_and(|execution| execution.status == "running")
                            {
                                if presentation
                                    .as_ref()
                                    .is_some_and(|metadata| metadata.started_at.is_none())
                                {
                                    "queued"
                                } else {
                                    "running"
                                }
                            } else if executions
                                .get(&event.execution_id)
                                .is_some_and(|execution| execution.status == "cancelled")
                            {
                                "cancelled"
                            } else {
                                "unknown"
                            }
                            .into(),
                            started: Instant::now(),
                            seconds: presentation.as_ref().and_then(|metadata| {
                                Some(
                                    (metadata.finished_at? - metadata.started_at?).max(0) as f64
                                        / 1000.0,
                                )
                            }),
                            activity: presentation.as_ref().map(|metadata| metadata.activity),
                            source: presentation
                                .map(|metadata| metadata.source)
                                .unwrap_or_default(),
                            output_bytes: None,
                            summary,
                            error: None,
                        });
                    }
                    HistoryContent::ToolResult {
                        call_id,
                        name,
                        status,
                        output,
                        output_bytes,
                        ..
                    } => {
                        let key = (event.execution_id.clone(), call_id.clone());
                        if let Some(index) = calls.get(&key).copied() {
                            if let Entry::Tool {
                                status: old,
                                output_bytes: bytes,
                                error,
                                ..
                            } = &mut self.entries[index]
                            {
                                let cancelled =
                                    cancelled_calls.contains(&key) && status != "success";
                                *old = if cancelled {
                                    "cancelled".into()
                                } else {
                                    status.clone()
                                };
                                *bytes = Some(output_bytes);
                                *error = (status != "success" && !cancelled).then(|| {
                                    output.lines().next().unwrap_or("Tool failed").to_owned()
                                });
                            }
                        } else {
                            self.entries.push(Entry::Tool {
                                id: call_id,
                                name,
                                status: status.clone(),
                                started: Instant::now(),
                                seconds: None,
                                activity: None,
                                source: ToolSource::Builtin,
                                output_bytes: Some(output_bytes),
                                summary: None,
                                error: (status != "success").then_some(output),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(execution) = previous.as_ref().and_then(|id| executions.get(id)) {
            self.history_timing(execution);
        }
        for (execution, index) in last_assistant {
            if with_tools.contains(&execution) {
                if let Entry::Assistant(cell) = &mut self.entries[index] {
                    cell.final_separator = true;
                }
            }
        }
        self.settle_compactions(false);
        crate::questions::history(self);
        self.transcript_version += 1;
        self.assistant = None;
        self.step = None;
        self.phase = Phase::Ready;
        self.dirty = true;
    }
    fn history_timing(&mut self, execution: &HistoryExecution) {
        if let Some(finished) = execution.finished_at {
            self.entries.push(Entry::RunTiming {
                label: match execution.status.as_str() {
                    "completed" => "Completed",
                    "cancelled" => "Stopped",
                    _ => "Failed",
                }
                .into(),
                seconds: (finished - execution.started_at).max(0) as u64 / 1000,
            });
        }
    }

    pub fn begin(&mut self) -> Option<Submission> {
        if self.phase != Phase::Ready || self.model_busy || self.permissions_busy || self.image_busy
        {
            return None;
        }
        let input = self.composer.lines().join("\n");
        if input.trim().is_empty() && self.attachments.is_empty() {
            return None;
        }
        let submission = Submission {
            resume: input.trim() == "/resume",
            attachments: std::mem::take(&mut self.attachments),
            input: input.clone(),
        };
        self.transcript_version += 1;
        self.entries.push(Entry::Message {
            role: "user".into(),
            content: std::iter::once(input)
                .filter(|text| !text.is_empty())
                .chain(
                    submission
                        .attachments
                        .iter()
                        .map(|image| format!("[Attachment: {}]", image.name)),
                )
                .collect::<Vec<_>>()
                .join("\n"),
        });
        self.composer = composer::new();
        self.draft_epoch += 1;
        self.pending = Some(submission.clone());
        self.active_permissions = None;
        self.active_model = None;
        self.run_started = Some(Instant::now());
        self.phase = Phase::Submitting;
        self.run = None;
        self.cursor = 0;
        self.assistant = None;
        self.step = None;
        self.cancel_requested = false;
        self.cancel_deadline = None;
        self.follow = true;
        self.dirty = true;
        Some(submission)
    }

    pub fn restore_failed_submission(&mut self) -> bool {
        if self.image_busy
            || !self.attachments.is_empty()
            || self.composer.lines().iter().any(|line| !line.is_empty())
        {
            return false;
        }
        let Some(submission) = self.failed_submission.take() else {
            return false;
        };
        self.composer = composer::new();
        self.composer.insert_str(if submission.resume {
            "/resume"
        } else {
            &submission.input
        });
        self.attachments = submission.attachments;
        self.draft_epoch += 1;
        self.dirty = true;
        true
    }
    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.max_scroll);
        self.follow = self.scroll == self.max_scroll;
        self.dirty = true;
    }
}
