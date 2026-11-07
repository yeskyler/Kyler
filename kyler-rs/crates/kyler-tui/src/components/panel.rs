//! Shared composer-anchored surface for commands, settings and detail views.
use crate::components::{composer::MESSAGE_BACKGROUND, transcript};

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::{Block, BorderType, Clear, Paragraph},
};

pub enum Position {
    Selected(usize),
    Scroll(usize),
}
pub fn selected(text: String, active: bool) -> Line<'static> {
    Line::styled(
        text,
        if active {
            Style::default().fg(Color::White).bg(MESSAGE_BACKGROUND)
        } else {
            Style::default().fg(Color::Gray)
        },
    )
}
pub fn draw(
    frame: &mut Frame,
    input: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    hint: &str,
    wide: bool,
    position: Position,
) -> (Rect, usize) {
    let screen = frame.area();
    let available = input.y.saturating_sub(screen.y);
    let width = input.width.min(if wide { input.width } else { 52 });
    let lines = if wide {
        transcript::wrap(lines, width.saturating_sub(2))
    } else {
        lines
    };
    let max_height = (available / 2).max(3).min(available);
    let height = (lines.len().saturating_add(3).min(u16::MAX as usize) as u16).min(max_height);
    let area = Rect::new(input.x, input.y.saturating_sub(height), width, height);
    if area.is_empty() {
        return (area, 0);
    }
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(format!(" {title} "));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let hint_rows = u16::from(inner.height > 1 && !hint.is_empty());
    let rows = inner.height.saturating_sub(hint_rows) as usize;
    let offset = match position {
        Position::Selected(index) => index.saturating_sub(rows.saturating_sub(1)),
        Position::Scroll(offset) => offset,
    }
    .min(lines.len().saturating_sub(rows));
    frame.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(offset)
                .take(rows)
                .collect::<Vec<_>>(),
        ),
        Rect::new(inner.x, inner.y, inner.width, rows as u16),
    );
    if hint_rows > 0 {
        frame.render_widget(
            Paragraph::new(hint.to_owned()).style(Style::default().fg(Color::DarkGray)),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
    (area, offset)
}
