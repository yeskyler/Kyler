use crate::{
    components::{
        composer::MESSAGE_BACKGROUND,
        header,
        panel::{self, Position},
        status, transcript,
    },
    state::{Phase, State},
};

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

#[derive(Default)]
pub struct View {
    version: Option<u64>,
    width: u16,
    welcome_height: usize,
    unread: bool,
    lines: Vec<Line<'static>>,
    keys: Vec<u64>,
    starts: Vec<usize>,
    pub input: Rect,
    pub panel: Rect,
    pub transcript: Rect,
}
impl View {
    pub fn render(&mut self, frame: &mut Frame, state: &mut State) {
        self.panel = Rect::default();
        let screen = frame.area();
        if screen.width < 20 || screen.height < 12 {
            frame.render_widget(Paragraph::new("Resize terminal (at least 20x12)"), screen);
            return;
        }
        let width = screen.width.saturating_sub(4);
        let area = Rect::new(
            screen.x + (screen.width - width) / 2,
            screen.y + 1,
            width,
            screen.height - 2,
        );
        let welcome = header::lines(state, width);
        let anchor = if !state.follow && self.width != width && state.scroll >= self.welcome_height
        {
            let row = state.scroll - self.welcome_height;
            self.starts
                .iter()
                .rposition(|start| *start <= row)
                .map(|index| {
                    (
                        index,
                        row.saturating_sub(self.starts[index])
                            .saturating_mul(usize::from(self.width))
                            / usize::from(width.max(1)),
                    )
                })
        } else {
            None
        };
        if state.follow {
            self.unread = false;
        } else if self.version.is_some() && self.version != Some(state.transcript_version) {
            self.unread = true;
        }
        if self.version != Some(state.transcript_version) || self.width != width {
            if self.width != width {
                self.keys.clear();
                self.starts.clear();
            }
            let keys = state
                .entries
                .iter()
                .map(transcript::fingerprint)
                .collect::<Vec<_>>();
            let mut common = keys
                .iter()
                .zip(&self.keys)
                .take_while(|(a, b)| a == b)
                .count();
            // Tool entries share a category tree. Rebuild the complete preceding group when
            // a call changes or a new entry is appended to it.
            while common > 0
                && matches!(state.entries[common - 1], crate::state::Entry::Tool { .. })
            {
                common -= 1;
            }
            let start = if common == 0 {
                0
            } else {
                self.starts.get(common).copied().unwrap_or(self.lines.len())
            };
            self.lines.truncate(start);
            self.keys.truncate(common);
            self.starts.truncate(common);
            let mut index = common;
            while index < state.entries.len() {
                let start = self.lines.len();
                if matches!(state.entries[index], crate::state::Entry::Tool { .. }) {
                    let end = state.entries[index..]
                        .iter()
                        .position(|entry| !matches!(entry, crate::state::Entry::Tool { .. }))
                        .map_or(state.entries.len(), |offset| index + offset);
                    self.lines
                        .extend(transcript::tool_rows(&state.entries[index..end], width));
                    for key in &keys[index..end] {
                        self.starts.push(start);
                        self.keys.push(*key);
                    }
                    index = end;
                } else {
                    self.starts.push(start);
                    self.keys.push(keys[index]);
                    self.lines.extend(transcript::wrap(
                        transcript::lines_with_width(
                            std::slice::from_ref(&state.entries[index]),
                            width,
                        ),
                        width,
                    ));
                    index += 1;
                }
            }
            if state.entries.is_empty() {
                self.lines = transcript::wrap(transcript::lines(&[]), width);
            }
            self.version = Some(state.transcript_version);
            self.width = width;
        }
        if let Some((index, offset)) = anchor {
            if let Some(start) = self.starts.get(index) {
                let end = self
                    .starts
                    .get(index + 1)
                    .copied()
                    .unwrap_or(self.lines.len());
                state.scroll = welcome.len() + start + offset.min(end.saturating_sub(*start + 1));
            }
        }
        self.welcome_height = welcome.len();
        let footer = transcript::wrap(vec![status::footer(state)], width);
        let footer_height = (footer.len() as u16).min(3);
        let input_width = width.saturating_sub(3).max(1) as usize;
        let rows = state
            .composer
            .lines()
            .iter()
            .map(|line| line.width().saturating_add(1).div_ceil(input_width).max(1))
            .sum::<usize>();
        let editor_rows = (rows.min(6) as u16).min((area.height / 3).max(1));
        let image_rows = (!state.attachments.is_empty() || state.image_busy) as u16;
        let editor_height = editor_rows + 2 + image_rows;
        let working = status::working(state);
        let running = if state.pending_approval().is_some() {
            Some("Waiting for approval · F2 to focus")
        } else if state.questions.pending().is_some() {
            Some("Waiting for input")
        } else {
            match state.phase {
                Phase::Loading => Some("Connecting…"),
                Phase::Submitting | Phase::Running => Some(working.as_str()),
                Phase::Cancelling => Some("Cancelling…"),
                Phase::Detached => Some("Disconnected · F5 retry / reconnect"),
                Phase::Ready => None,
            }
        };
        let status_height = u16::from(running.is_some());
        let available = area
            .height
            .saturating_sub(editor_height + footer_height + status_height)
            .max(1);
        let content_len = welcome.len() + self.lines.len();
        let mut count = content_len;
        if !state.follow {
            count += 1;
        }
        let history_height = available;
        self.transcript = Rect::new(area.x, area.y, width, history_height);
        state.max_scroll = count.saturating_sub(history_height as usize);
        if state.follow {
            state.scroll = state.max_scroll;
        } else {
            state.scroll = state.scroll.min(state.max_scroll);
        }
        let mut visible = welcome
            .iter()
            .chain(self.lines.iter())
            .skip(state.scroll)
            .take(history_height as usize)
            .cloned()
            .collect::<Vec<_>>();
        if !state.follow {
            if visible.len() >= history_height as usize {
                visible.pop();
            }
            visible.push(Line::styled(
                if self.unread {
                    "↓ New activity · Ctrl+End"
                } else {
                    "↓ Latest · Ctrl+End"
                },
                Style::default().fg(Color::Cyan),
            ));
        }
        frame.render_widget(Paragraph::new(visible), self.transcript);
        if let Some(message) = running {
            frame.render_widget(
                Paragraph::new(message).style(Style::default().fg(Color::DarkGray)),
                Rect::new(area.x, self.transcript.bottom(), width, status_height),
            );
        }
        self.input = Rect::new(
            area.x,
            self.transcript.bottom() + status_height,
            width,
            editor_height,
        );
        let editor = Rect::new(
            self.input.x,
            self.input.y + image_rows,
            self.input.width,
            self.input.height.saturating_sub(image_rows),
        );
        frame.render_widget(&state.composer, editor);
        frame
            .buffer_mut()
            .set_style(self.input, Style::default().bg(MESSAGE_BACKGROUND));
        if image_rows > 0 {
            let label = if state.image_busy {
                "  Reading attachment…".to_string()
            } else {
                format!(
                    "  {}  · Alt+Backspace remove",
                    state
                        .attachments
                        .iter()
                        .map(|image| format!("[Attachment: {}]", image.name))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            };
            frame.render_widget(
                Paragraph::new(label).style(Style::default().bg(MESSAGE_BACKGROUND)),
                Rect::new(self.input.x, self.input.y, width, 1),
            );
        }
        frame.render_widget(
            Paragraph::new("> ").style(Style::default().bg(MESSAGE_BACKGROUND)),
            Rect::new(self.input.x, self.input.y + image_rows + 1, 2, 1),
        );
        let cursor = state.composer.screen_cursor();
        let (top, left) = state.composer.scroll_offset();
        let row = cursor.row.saturating_sub(top as usize);
        let col = cursor.col.saturating_sub(left as usize);
        if row < editor_rows as usize
            && col < input_width
            && !state.panel_open()
            && !(state.approval_focus && state.pending_approval().is_some())
            && crate::components::command_picker::matches(state).is_empty()
        {
            frame.set_cursor_position((
                self.input.x + 2 + col as u16,
                self.input.y + image_rows + 1 + row as u16,
            ));
        }
        frame.render_widget(
            Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
            Rect::new(area.x, self.input.bottom(), width, footer_height),
        );
        // Render one surface, always anchored to the same composer edge.
        self.panel = if state.questions.pending().is_some()
            && state.pending_approval().is_none()
            && !state.confirm_exit
        {
            crate::questions::render(frame, state, self.input)
        } else if let Some(details) = &state.tool_details {
            let text = format!(
                "{} · {}\nArguments\n{}\nOutput ({} bytes{})\n{}",
                details.name,
                details.status,
                serde_json::to_string_pretty(&details.arguments).unwrap_or_default(),
                details.output_bytes,
                if details.truncated { ", truncated" } else { "" },
                details.output
            );
            let lines = transcript::wrap(
                text.lines()
                    .map(|line| Line::from(crate::components::composer::clean(line)))
                    .collect(),
                width.saturating_sub(4),
            );
            let (area, scroll) = crate::components::panel::draw(
                frame,
                self.input,
                "Tool details",
                lines,
                "↑↓ Scroll · Esc Close",
                true,
                Position::Scroll(state.tool_scroll),
            );
            state.tool_scroll = scroll;
            area
        } else if state.confirm_exit || state.help {
            let (title, text, hint) = if state.confirm_exit {
                (
                    "Exit",
                    "Exit TUI? Active tasks keep running. Drafts are not saved.",
                    "Enter Exit · Esc Back",
                )
            } else {
                (
                    "Help",
                    "/model        Model\n/permissions  Permissions\n/mcp          MCP\n/skills       Skills\n$             Select skill\n@PATH         Attach file\nCtrl+V        Paste image / text\nAlt+Backspace Remove image\nEnter         Send\nAlt+Enter     New line\nCtrl+P        Permissions\nF2            Approvals\nF3            Latest tool details\nPageUp/Down   Scroll messages\nCtrl+End      Latest message\nCtrl+N        New session\nF5            Reconnect\nCtrl+C        Cancel / exit\nCtrl+R        Restore failed input\nF10           Exit",
                    "↑↓ Scroll · Esc Back",
                )
            };
            let (area, scroll) = panel::draw(
                frame,
                self.input,
                title,
                text.lines()
                    .map(|line| Line::from(line.to_owned()))
                    .collect(),
                hint,
                state.confirm_exit,
                Position::Scroll(state.help_scroll),
            );
            state.help_scroll = scroll;
            area
        } else if state.approval_details.is_some() {
            crate::components::permission_picker::render_approval(frame, state, self.input)
        } else if state.model_open {
            crate::components::model_picker::render(frame, state, self.input)
        } else if state.permissions_open {
            crate::components::permission_picker::render(frame, state, self.input)
        } else if state.skills_open {
            crate::components::skills_panel::render(frame, state, self.input)
        } else if state.mcp_open {
            crate::components::mcp_panel::render(frame, state, self.input)
        } else if state.pending_approval().is_some() {
            crate::components::approval::render(frame, state, self.input)
        } else if !crate::components::skill_picker::candidates(state).is_empty() {
            crate::components::skill_picker::render(frame, state, self.input)
        } else if !crate::components::attachment_picker::candidates(state).is_empty() {
            crate::components::attachment_picker::render(frame, state, self.input)
        } else {
            crate::components::command_picker::render(frame, state, self.input)
        };
    }
}
