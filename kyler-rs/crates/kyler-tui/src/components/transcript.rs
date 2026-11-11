use crate::{components::composer::clean, state::Entry};

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use kyler_protocol::activity::ToolSource;

/// Wrap display cells without splitting a grapheme or losing whitespace in code blocks.
pub fn wrap(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let mut out = Vec::new();
    for line in lines {
        let style = line.style;
        let mut spans = Vec::new();
        let mut used = 0;
        for span in line.spans {
            let mut chunk = String::new();
            for glyph in span.content.graphemes(true) {
                let n = glyph.width();
                if used + n > width && used > 0 {
                    if !chunk.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut chunk), span.style));
                    }
                    out.push(display_row(std::mem::take(&mut spans), style, width, used));
                    used = 0;
                }
                if n > width {
                    chunk.push('�');
                    used += 1;
                } else {
                    chunk.push_str(glyph);
                    used += n;
                }
            }
            if !chunk.is_empty() {
                spans.push(Span::styled(chunk, span.style));
            }
        }
        out.push(display_row(spans, style, width, used));
    }
    out
}
fn display_row(
    mut spans: Vec<Span<'static>>,
    style: Style,
    width: usize,
    used: usize,
) -> Line<'static> {
    if style.bg.is_some() {
        spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    }
    Line::from(spans).style(style)
}

pub fn lines(entries: &[Entry]) -> Vec<Line<'static>> {
    lines_with_width(entries, 80)
}

pub fn lines_with_width(entries: &[Entry], width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if matches!(entry, Entry::Tool { .. }) {
            if index > 0 && matches!(entries[index - 1], Entry::Tool { .. }) {
                continue;
            }
            let end = entries[index..]
                .iter()
                .position(|entry| !matches!(entry, Entry::Tool { .. }))
                .map_or(entries.len(), |offset| index + offset);
            lines.extend(tool_rows(&entries[index..end], width));
            continue;
        }
        match entry {
            Entry::RunTiming { label, seconds } => {
                lines.push(Line::styled(
                    format!("  {label} {}", super::status::duration(*seconds)),
                    Style::default().fg(Color::DarkGray),
                ));
                lines.push(Line::default());
            }
            Entry::Error { title, message } => {
                lines.push(Line::styled(
                    format!("{}: {}", clean(title), clean(message)),
                    Style::default().fg(Color::Red),
                ));
                lines.push(Line::default());
            }
            Entry::Approval { .. } => {}
            Entry::Assistant(cell) => {
                if cell.final_separator {
                    lines.push(Line::styled(
                        format!(
                            "  {}",
                            "─".repeat(usize::from(width.saturating_sub(2)).min(36))
                        ),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                lines.extend(super::markdown::render(cell.displayed(), width));
                if !cell.displayed().trim().is_empty() {
                    lines.push(Line::default());
                }
            }
            Entry::Reasoning(cell) => {
                lines.extend(reasoning_rows(cell, width));
                lines.push(Line::default());
            }
            Entry::Message { role, content } => {
                if role == "assistant" {
                    lines.extend(super::markdown::render(content, width));
                } else {
                    lines.extend(super::messages::user::render(content, width));
                }
                lines.push(Line::default());
            }
            Entry::Tool { .. } => unreachable!("tool groups are rendered above"),
            Entry::Compaction(info) => {
                lines.push(Line::styled(
                    format!("── {} ──", info.status.label()),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            Entry::Notice(text) => {
                lines.push(Line::styled(
                    clean(&format!("  · {text}")),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }
    }
    lines
}

fn reasoning_rows(
    cell: &super::messages::assistant::ReasoningCell,
    width: u16,
) -> Vec<Line<'static>> {
    let label = if cell.streaming {
        "Thinking…".to_owned()
    } else if let Some(seconds) = cell.seconds {
        format!(
            "Thought for {}",
            if seconds == 0 {
                "<1s".into()
            } else {
                super::status::duration(seconds)
            }
        )
    } else {
        "Thought".to_owned()
    };
    let mut rows = wrap(
        vec![Line::styled(
            format!("  {} {label} · Ctrl+O", if cell.open() { "▾" } else { "▸" }),
            Style::default().fg(Color::DarkGray),
        )],
        width,
    );
    if !cell.open() {
        return rows;
    }
    let inner_width = width.saturating_sub(4).max(1);
    let body = wrap(
        super::markdown::render_reasoning(&cell.content, inner_width),
        inner_width,
    );
    let preview = cell.streaming && cell.expanded != Some(true);
    let skip = if preview {
        body.len().saturating_sub(6)
    } else {
        0
    };
    for line in body.into_iter().skip(skip) {
        let mut spans = vec![Span::raw("  │ ")];
        spans.extend(line.spans.into_iter().map(|mut span| {
            span.style = span.style.fg(Color::DarkGray);
            span
        }));
        rows.push(Line::from(spans).style(Style::default().fg(Color::DarkGray)));
    }
    rows
}

/// Group consecutive calls by category without changing execution order.
pub fn tool_rows(entries: &[Entry], width: u16) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut index = 0;
    while index < entries.len() {
        let Some(category) = tool_category(&entries[index]) else {
            index += 1;
            continue;
        };
        let end = entries[index..]
            .iter()
            .position(|entry| tool_category(entry) != Some(category))
            .map_or(entries.len(), |offset| index + offset);
        let group = &entries[index..end];
        let failed = group.iter().any(|entry| matches!(entry, Entry::Tool { status, .. } if tool_status(status).1 == Color::Red));
        let blocked = group.iter().any(|entry| matches!(entry, Entry::Tool { status, .. } if matches!(status.as_str(), "denied" | "permission_denied" | "pending" | "cancelled")));
        let running = group
            .iter()
            .any(|entry| matches!(entry, Entry::Tool { status, .. } if matches!(status.as_str(), "preparing" | "running")));
        let queued = group
            .iter()
            .any(|entry| matches!(entry, Entry::Tool { status, .. } if status == "queued"));
        let (symbol, color) = if failed {
            ("!", Color::Red)
        } else if running {
            ("◌", Color::Cyan)
        } else if blocked {
            ("–", Color::Yellow)
        } else if queued {
            ("○", Color::DarkGray)
        } else {
            ("✓", Color::Green)
        };
        rows.extend(wrap(
            vec![Line::from(vec![
                Span::styled(format!("{symbol} "), Style::default().fg(color)),
                Span::styled(
                    category,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        " · {} {}",
                        group.len(),
                        if group.len() == 1 { "call" } else { "calls" }
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
            ])],
            width,
        ));
        for (offset, entry) in group.iter().enumerate() {
            let last = offset + 1 == group.len();
            let (branch, continuation) = if width >= 6 {
                (
                    if last { "  └─ " } else { "  ├─ " },
                    if last { "     " } else { "  │  " },
                )
            } else if width >= 3 {
                (
                    if last { "└ " } else { "├ " },
                    if last { "  " } else { "│ " },
                )
            } else {
                ("", "")
            };
            let available = width.saturating_sub(branch.width() as u16).max(1);
            for (line_index, line) in wrap(
                vec![tool_line(entry).expect("tool category requires a tool entry")],
                available,
            )
            .into_iter()
            .enumerate()
            {
                let mut spans = vec![Span::styled(
                    if line_index == 0 {
                        branch
                    } else {
                        continuation
                    },
                    Style::default().fg(Color::DarkGray),
                )];
                spans.extend(line.spans);
                rows.push(Line::from(spans));
            }
            if let Entry::Tool {
                error: Some(error), ..
            } = entry
            {
                let text = clean(error)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                for line in wrap(
                    vec![Line::styled(text, Style::default().fg(Color::Red))],
                    available,
                ) {
                    let mut spans = vec![Span::styled(
                        continuation,
                        Style::default().fg(Color::DarkGray),
                    )];
                    spans.extend(line.spans);
                    rows.push(Line::from(spans));
                }
            }
        }
        index = end;
    }
    if !rows.is_empty() {
        rows.push(Line::default());
    }
    rows
}

fn tool_category(entry: &Entry) -> Option<&'static str> {
    let Entry::Tool {
        activity, source, ..
    } = entry
    else {
        return None;
    };
    Some(
        if matches!(source, kyler_protocol::activity::ToolSource::Skill { .. }) {
            "Skill"
        } else {
            activity.map(|activity| activity.label()).unwrap_or("Tool")
        },
    )
}

fn tool_status(status: &str) -> (&'static str, Color, &'static str) {
    match status {
        "success" => ("✓", Color::Green, ""),
        "unknown" => ("?", Color::DarkGray, "Outcome unknown · F3 details"),
        "preparing" => ("◌", Color::Cyan, "Preparing call"),
        "queued" => ("○", Color::DarkGray, "Queued"),
        "running" => ("◌", Color::Cyan, ""),
        "cancelled" => ("–", Color::DarkGray, "Cancelled"),
        "denied" | "permission_denied" => ("–", Color::Yellow, "Permission denied"),
        "pending" => ("?", Color::Yellow, "Awaiting approval"),
        _ => ("!", Color::Red, "Failed · F3 details"),
    }
}

fn tool_line(entry: &Entry) -> Option<Line<'static>> {
    let Entry::Tool {
        name,
        status,
        seconds,
        source,
        output_bytes,
        summary,
        ..
    } = entry
    else {
        return None;
    };
    let (symbol, color, label) = tool_status(status);
    let display_name = match source {
        ToolSource::Builtin => name.clone(),
        ToolSource::Skill { name, .. } => name.clone(),
        ToolSource::Mcp {
            server_id,
            remote_name,
        } => format!("{server_id}/{remote_name}"),
    };
    let mut spans = vec![
        Span::styled(format!("{symbol} "), Style::default().fg(color)),
        Span::raw(clean(&display_name)),
    ];
    if let Some(summary) = summary.as_ref().filter(|summary| !summary.is_empty()) {
        spans.push(Span::styled(
            format!("  {}", clean(summary)),
            Style::default().fg(Color::Gray),
        ));
    }
    if !label.is_empty() {
        spans.push(Span::styled(
            format!(" · {label}"),
            Style::default().fg(color),
        ));
    }
    let mut details = Vec::new();
    if let Some(seconds) = seconds {
        details.push(format!("{seconds:.1}s"));
    }
    if let Some(bytes) = output_bytes {
        details.push(if *bytes < 1024 {
            format!("{bytes} B")
        } else if *bytes < 1024 * 1024 {
            format!("{:.1} KB", *bytes as f64 / 1024.0)
        } else {
            format!("{:.1} MB", *bytes as f64 / (1024.0 * 1024.0))
        });
    }
    if !details.is_empty() {
        spans.push(Span::styled(
            format!(" · {}", details.join(" · ")),
            Style::default().fg(Color::DarkGray),
        ));
    }
    Some(Line::from(spans))
}

/// Stable per-entry identity lets the view retain already-wrapped history while streaming.
pub fn fingerprint(entry: &Entry) -> u64 {
    let mut hash = DefaultHasher::new();
    match entry {
        Entry::Assistant(cell) => {
            7u8.hash(&mut hash);
            cell.displayed().hash(&mut hash);
            cell.is_finished().hash(&mut hash);
            cell.final_separator.hash(&mut hash);
        }
        Entry::Reasoning(cell) => {
            8u8.hash(&mut hash);
            cell.content.hash(&mut hash);
            cell.streaming.hash(&mut hash);
            cell.expanded.hash(&mut hash);
            cell.seconds.hash(&mut hash);
        }
        Entry::RunTiming { label, seconds } => {
            6u8.hash(&mut hash);
            label.hash(&mut hash);
            seconds.hash(&mut hash);
        }
        Entry::Error { title, message } => {
            4u8.hash(&mut hash);
            title.hash(&mut hash);
            message.hash(&mut hash);
        }
        Entry::Approval {
            record,
            expanded,
            choice,
        } => {
            3u8.hash(&mut hash);
            record.id.hash(&mut hash);
            record.preview.hash(&mut hash);
            record.reason.hash(&mut hash);
            format!("{:?}", record.status).hash(&mut hash);
            expanded.hash(&mut hash);
            choice.hash(&mut hash);
        }
        Entry::Message { role, content } => {
            0u8.hash(&mut hash);
            role.hash(&mut hash);
            content.hash(&mut hash);
        }
        Entry::Tool {
            id,
            name,
            status,
            seconds,
            activity,
            source,
            output_bytes,
            summary,
            error,
            ..
        } => {
            1u8.hash(&mut hash);
            id.hash(&mut hash);
            name.hash(&mut hash);
            activity.hash(&mut hash);
            source.hash(&mut hash);
            output_bytes.hash(&mut hash);
            summary.hash(&mut hash);
            error.hash(&mut hash);
            status.hash(&mut hash);
            seconds.map(f64::to_bits).hash(&mut hash);
        }
        Entry::Compaction(info) => {
            7u8.hash(&mut hash);
            info.id.hash(&mut hash);
            format!("{:?}", info.status).hash(&mut hash);
        }
        Entry::Notice(text) => {
            2u8.hash(&mut hash);
            text.hash(&mut hash);
        }
    }
    hash.finish()
}
