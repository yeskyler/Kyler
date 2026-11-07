use crate::{components::composer::clean, state::State};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Welcome card belongs to the transcript, so it leaves the screen with old messages.
pub fn lines(state: &State, width: u16) -> Vec<Line<'static>> {
    let card_width = usize::from(width.min(72));
    let inner = card_width.saturating_sub(2);
    let margin = "";
    let border = Style::default().fg(Color::DarkGray);
    let title = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(Color::DarkGray);
    let model = state.displayed_model();
    let workspace = state
        .info
        .as_ref()
        .and_then(|i| {
            i.working_directory
                .as_deref()
                .or(i.workspace_root.as_deref())
        })
        .unwrap_or("Waiting for session workspace");
    let edge = |left: &str, right: &str| {
        Line::styled(
            format!("{margin}{left}{}{right}", "┄".repeat(inner)),
            border,
        )
    };
    let row = |text: &str, style: Style| {
        let padding = inner.min(2);
        let text = fit(text, inner.saturating_sub(padding * 2));
        let spare = inner.saturating_sub(text.width());
        let left = spare.min(padding);
        Line::from(vec![
            Span::styled(format!("{margin}┆"), border),
            Span::styled(
                format!("{}{text}{}", " ".repeat(left), " ".repeat(spare - left)),
                style,
            ),
            Span::styled("┆", border),
        ])
    };
    let label = fit(" kyler ", inner.saturating_sub(2));
    let lead = inner.saturating_sub(label.width()).min(2);
    let top = Line::from(vec![
        Span::styled(format!("╭{}", "┄".repeat(lead)), border),
        Span::styled(label.clone(), title),
        Span::styled(
            format!("{}╮", "┄".repeat(inner - lead - label.width())),
            border,
        ),
    ]);
    vec![
        top,
        row("", muted),
        row(&format!("Model  {}", clean(model)), Style::default()),
        row("", muted),
        row(&format!("Directory  {}", clean(workspace)), muted),
        row("", muted),
        edge("╰", "╯"),
        Line::default(),
    ]
}
fn fit(text: &str, width: usize) -> String {
    let text = clean(text).replace('\n', " ");
    if text.width() <= width {
        return text;
    }
    if width == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut used = 0;
    for glyph in text.graphemes(true) {
        if used + glyph.width() >= width {
            break;
        }
        result.push_str(glyph);
        used += glyph.width();
    }
    result.push('…');
    result
}
