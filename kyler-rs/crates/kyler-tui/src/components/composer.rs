use ratatui::{
    style::{Color, Style},
    widgets::{Block, Padding},
};
use ratatui_textarea::{TextArea, WrapMode};

pub const MESSAGE_BACKGROUND: Color = Color::Rgb(55, 60, 70);

pub fn new() -> TextArea<'static> {
    let mut editor = TextArea::default();
    editor.set_placeholder_text("Type a message…");
    editor.set_placeholder_style(Style::default().fg(Color::DarkGray).bg(MESSAGE_BACKGROUND));
    editor.set_wrap_mode(WrapMode::Glyph);
    editor.set_style(Style::default().bg(MESSAGE_BACKGROUND));
    editor.set_cursor_line_style(Style::default().bg(MESSAGE_BACKGROUND));
    editor.set_cursor_style(Style::default().bg(MESSAGE_BACKGROUND));
    editor.set_block(
        Block::default()
            .style(Style::default().bg(MESSAGE_BACKGROUND))
            .padding(Padding::new(2, 1, 1, 1)),
    );
    editor
}
pub fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect::<String>()
        .replace('\t', "    ")
}
