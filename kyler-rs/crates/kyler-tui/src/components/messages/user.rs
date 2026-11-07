//! User text and attachment placeholders are literal, never Markdown.
use crate::components::{
    composer::{MESSAGE_BACKGROUND, clean},
    transcript::wrap,
};

use ratatui::{
    style::Style,
    text::{Line, Span},
};

pub fn render(content: &str, width: u16) -> Vec<Line<'static>> {
    let background = Style::default().bg(MESSAGE_BACKGROUND);
    let mut rows = vec![Line::styled("", background)];
    for (index, text) in clean(content).split('\n').enumerate() {
        for (row, line) in wrap(
            vec![Line::from(text.to_owned())],
            width.saturating_sub(2).max(1),
        )
        .into_iter()
        .enumerate()
        {
            let prefix = if index == 0 && row == 0 { "> " } else { "  " };
            let mut spans = vec![Span::raw(prefix)];
            spans.extend(line.spans);
            rows.push(Line::from(spans).style(background));
        }
    }
    rows.push(Line::styled("", background));
    rows
}
