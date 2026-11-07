//! Composer-anchored approval interaction and bounded argument previews.
use crate::{
    components::composer::clean,
    state::{Entry, State},
};

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph},
};
use serde_json::Value;

use kyler_protocol::api::permissions::ApprovalRecord;

fn short(text: &str) -> String {
    let mut out: String = text.chars().take(160).collect();
    if text.chars().count() > 160 {
        out.push('…');
    }
    out
}

pub fn summary(record: &ApprovalRecord) -> Vec<String> {
    if let Ok(value) = serde_json::from_str::<Value>(&record.preview) {
        if record.tool == "save_skill" {
            if let Some(files) = value["files"].as_object() {
                let action = if value["expected_fingerprint"].is_string() {
                    "Update"
                } else {
                    "Create"
                };
                let scope = value["scope"].as_str().unwrap_or("user");
                let name = value["name"].as_str().unwrap_or("skill");
                let bytes: usize = files.values().filter_map(Value::as_str).map(str::len).sum();
                let mut lines = vec![short(&format!(
                    "{action} {scope} skill: {name} · {} files · {bytes} bytes",
                    files.len()
                ))];
                lines.extend(files.keys().take(3).map(|path| short(&format!("  {path}"))));
                if files.len() > 3 {
                    lines.push(format!("  +{} more files", files.len() - 3));
                }
                return lines;
            }
        }
        if record.tool == "install_skill" {
            return vec![
                format!(
                    "Install {} skill",
                    value["scope"].as_str().unwrap_or("user")
                ),
                short(value["path"].as_str().unwrap_or("Local package")),
            ];
        }
    }
    record.preview.lines().take(3).map(short).collect()
}

pub fn details(record: &ApprovalRecord) -> Vec<String> {
    if record.tool == "save_skill" {
        if let Ok(Value::Object(mut args)) = serde_json::from_str::<Value>(&record.preview) {
            if let Some(Value::Object(files)) = args.remove("files") {
                let mut lines = serde_json::to_string_pretty(&args)
                    .unwrap_or_default()
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                for (path, content) in files {
                    lines.push(format!("File: {path}"));
                    match content {
                        Value::String(text) => lines.extend(text.lines().map(str::to_owned)),
                        other => lines.push(other.to_string()),
                    }
                }
                return lines;
            }
        }
    }
    record.preview.lines().map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::{details, summary};

    use kyler_protocol::api::permissions::{
        ApprovalRecord, ApprovalStatus, PermissionMode, PermissionSnapshot,
    };

    #[test]
    fn skill_summary_is_bounded_and_details_keep_all_file_content() {
        let record = ApprovalRecord {
            id: "approval".into(), run_id: "run".into(), session_id: "session".into(), origin_session_id: "session".into(), execution_id: "execution".into(), tool_call_id: "call".into(),
            tool: "save_skill".into(), operation: "Modify files".into(),
            preview: serde_json::json!({"name":"support","scope":"user","expected_fingerprint":"revision","files":{"SKILL.md":"line one\nline two"}}).to_string(),
            requires_user: true, working_directory: None, policy: PermissionSnapshot { approval: None, mode: PermissionMode::AutoReview, revision: "revision".into() }, digest: "digest".into(), status: ApprovalStatus::Pending, reason: "Review".into(), created_at: 0, expires_at: 0,
        };
        assert!(summary(&record)[0].starts_with("Update user skill: support"));
        assert!(!summary(&record).join("\n").contains("line one"));
        let full = details(&record).join("\n");
        assert!(full.contains("line one\nline two"));
        assert!(full.contains("expected_fingerprint"));
        assert!(full.contains("File: SKILL.md"));
    }
}

/// Approval requests are an input interaction, never conversation history.
pub fn render(frame: &mut Frame, state: &State, input: Rect) -> Rect {
    let Some(index) = state.pending_approval() else {
        return Rect::default();
    };
    let Entry::Approval { record, choice, .. } = &state.entries[index] else {
        return Rect::default();
    };
    let available = input.y.saturating_sub(frame.area().y);
    let area = Rect::new(
        input.x,
        input.y.saturating_sub(available.min(12)),
        input.width,
        available.min(12),
    );
    if area.height < 3 {
        return area;
    }
    frame.render_widget(Clear, area);
    let pending = state.entries.iter().filter(|entry| matches!(entry, Entry::Approval { record, .. } if record.status == kyler_protocol::api::permissions::ApprovalStatus::Pending)).count();
    let title = if pending > 1 {
        format!(" Approval · {pending} pending ")
    } else {
        " Approval ".into()
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let stacked = inner.width < 24 && inner.height >= 2;
    let buttons = if stacked { 2 } else { 1 };
    let hint_rows = u16::from(inner.height > buttons + 1);
    let body_height = inner.height.saturating_sub(buttons + hint_rows);
    let mut body = vec![Line::styled(
        format!("{} · {}", clean(&record.tool), clean(&record.operation)),
        Style::default().fg(Color::Cyan),
    )];
    body.extend(
        summary(record)
            .into_iter()
            .map(|line| Line::from(clean(&line))),
    );
    if let Some(directory) = &record.working_directory {
        body.push(Line::from(format!("Directory: {}", clean(directory))));
    }
    body.push(Line::from(clean(&record.reason)));
    if let Some(error) = &state.approval_error {
        body.insert(
            1,
            Line::styled(clean(error), Style::default().fg(Color::Red)),
        );
    }
    let body = super::transcript::wrap(body, inner.width);
    frame.render_widget(
        Paragraph::new(body),
        Rect::new(inner.x, inner.y, inner.width, body_height),
    );
    let selected = Style::default().fg(Color::White).bg(Color::DarkGray);
    let normal = Style::default().fg(Color::Gray);
    let deny = Span::styled(
        " Deny ",
        if *choice != 0 && state.approval_focus {
            selected
        } else {
            normal
        },
    );
    let allow = Span::styled(
        " Allow once ",
        if *choice == 0 && state.approval_focus {
            selected
        } else {
            normal
        },
    );
    let button_y = inner.bottom().saturating_sub(buttons + hint_rows);
    if stacked {
        frame.render_widget(
            Paragraph::new(vec![Line::from(deny), Line::from(allow)]),
            Rect::new(inner.x, button_y, inner.width, buttons),
        );
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(vec![deny, Span::raw("  "), allow])),
            Rect::new(inner.x, button_y, inner.width, buttons),
        );
    }
    if hint_rows > 0 {
        let hint = if state.approval_busy {
            "Submitting decision…"
        } else if state.approval_focus {
            "←→ Select · Enter Confirm · y/n · e Details · Esc Input"
        } else {
            "F2 Review"
        };
        frame.render_widget(
            Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
    area
}
