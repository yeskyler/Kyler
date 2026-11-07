//! Explicit skill references complete into the draft without submitting a request.
use crate::state::State;

use ratatui::{Frame, layout::Rect};

use kyler_protocol::api::skills::{SkillInfo, SkillStatus};

pub fn query(state: &State) -> Option<(String, String)> {
    if state.panel_open() {
        return None;
    }
    let text = state.composer.lines().join("\n");
    if state.skill_dismissed.as_deref() == Some(&text) {
        return None;
    }
    let cursor = state.composer.cursor();
    let (row, column) = (cursor.0, cursor.1);
    if row + 1 != state.composer.lines().len()
        || column != state.composer.lines()[row].chars().count()
    {
        return None;
    }
    let start = text
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let token = text[start..].strip_prefix('$')?;
    if !token
        .bytes()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return None;
    }
    Some((text[..start].into(), token.into()))
}
pub fn candidates(state: &State) -> Vec<&SkillInfo> {
    let Some((_, query)) = query(state) else {
        return vec![];
    };
    state
        .skills
        .iter()
        .filter(|skill| skill.status == SkillStatus::Enabled && skill.name.starts_with(&query))
        .collect()
}
pub fn render(frame: &mut Frame, state: &State, input: Rect) -> Rect {
    let items = candidates(state);
    if items.is_empty() {
        return Rect::default();
    }
    let selected = state.skill_selected.min(items.len() - 1);
    let lines = items
        .iter()
        .enumerate()
        .map(|(index, skill)| {
            super::panel::selected(
                format!(
                    "{} ${}",
                    if index == selected { ">" } else { " " },
                    skill.name
                ),
                index == selected,
            )
        })
        .collect();
    super::panel::draw(
        frame,
        input,
        "Skills",
        lines,
        "↑↓ Select · Tab / Enter Insert · Esc Close",
        false,
        super::panel::Position::Selected(selected),
    )
    .0
}
