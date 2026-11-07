use crate::state::State;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use kyler_protocol::api::permissions::PermissionMode;

/// Fixed labels, not controls. The permission picker owns all changes.
pub fn footer(state: &State) -> Line<'static> {
    let selected = state
        .permissions
        .as_ref()
        .map(|state| state.selected.mode.preset());
    let mut spans = Vec::new();
    for mode in PermissionMode::ALL {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
        }
        let style = if selected == Some(mode) {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(mode.label(), style));
    }
    Line::from(spans)
}

#[cfg(test)]
pub fn text(state: &State) -> String {
    footer(state).to_string()
}

pub fn duration(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!(
            "{}h {:02}m {:02}s",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    }
}

pub fn working(state: &State) -> String {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let elapsed = state
        .run_started
        .map(|start| start.elapsed())
        .unwrap_or_default();
    format!(
        "{} working · {}",
        FRAMES[(elapsed.as_millis() / 100 % 10) as usize],
        duration(elapsed.as_secs())
    )
}
