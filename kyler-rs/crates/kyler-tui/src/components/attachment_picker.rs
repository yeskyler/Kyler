//! Local @ picker. Selected files are uploaded through the same API as web clients.
use crate::state::State;

use std::path::PathBuf;

use ratatui::{Frame, layout::Rect};

pub struct Candidate {
    pub path: PathBuf,
    pub label: String,
    pub directory: bool,
}
pub fn query(state: &State) -> Option<(String, String)> {
    if state.panel_open() {
        return None;
    }
    let text = state.composer.lines().join("\n");
    let start = text.rfind('@')?;
    if start > 0 && !text[..start].chars().last()?.is_whitespace() {
        return None;
    }
    let query = &text[start + 1..];
    if query.contains('\n') {
        return None;
    }
    Some((text[..start].into(), query.into()))
}
pub fn candidates(state: &State) -> Vec<Candidate> {
    let Some((_, query)) = query(state) else {
        return Vec::new();
    };
    let raw = if let Some(rest) = query.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(rest)
    } else {
        PathBuf::from(&query)
    };
    let path = if raw.is_absolute() {
        raw
    } else {
        std::env::current_dir().unwrap_or_default().join(raw)
    };
    let (directory, prefix) = if query.is_empty() || query.ends_with('/') {
        (path, String::new())
    } else {
        (
            path.parent().unwrap_or(&path).to_path_buf(),
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        )
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut items = entries
        .take(3000)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.to_lowercase().starts_with(&prefix.to_lowercase())
                || name.chars().any(char::is_control)
            {
                return None;
            }
            let directory = entry.file_type().ok()?.is_dir();
            Some(Candidate {
                path: entry.path(),
                label: format!("{name}{}", if directory { "/" } else { "" }),
                directory,
            })
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| b.directory.cmp(&a.directory).then(a.label.cmp(&b.label)));
    items.truncate(100);
    items
}
pub fn render(frame: &mut Frame, state: &State, input: Rect) -> Rect {
    let items = candidates(state);
    if items.is_empty() {
        return Rect::default();
    }
    let selected = state.at_selected.min(items.len() - 1);
    let lines = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            super::panel::selected(
                format!("{} {}", if i == selected { ">" } else { " " }, item.label),
                i == selected,
            )
        })
        .collect();
    super::panel::draw(
        frame,
        input,
        "Attachments",
        lines,
        "↑↓ Select · Enter Attach / Open · Esc Close",
        false,
        super::panel::Position::Selected(selected),
    )
    .0
}
