//! Local-only preview of OpenAI-compatible model configuration.
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::{Block, BorderType, Clear, Paragraph},
};
use ratatui_textarea::TextArea;

pub struct ModelConfigForm {
    fields: [TextArea<'static>; 3],
    focus: usize,
}

impl ModelConfigForm {
    pub fn new(model: String) -> Self {
        let mut fields = std::array::from_fn(|_| TextArea::default());
        fields[0].insert_str(model);
        fields[0].set_placeholder_text("Model ID");
        fields[1].set_placeholder_text("https://api.example.com/v1");
        fields[2].set_placeholder_text("Leave blank to keep existing key");
        fields[2].set_mask_char('•');
        Self { fields, focus: 0 }
    }

    pub fn key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab | KeyCode::Down | KeyCode::Enter => self.focus = (self.focus + 1) % 3,
            KeyCode::BackTab | KeyCode::Up => self.focus = (self.focus + 2) % 3,
            _ => {
                self.fields[self.focus].input(key);
            }
        }
    }

    pub fn paste(&mut self, text: &str) {
        let text = text.trim_end_matches(['\r', '\n']);
        if !text.chars().any(char::is_control) {
            self.fields[self.focus].insert_str(text);
        }
    }

    pub fn render(&self, frame: &mut Frame, input: Rect) -> Rect {
        let height = 13.min(input.y.saturating_sub(frame.area().y));
        let area = Rect::new(
            input.x,
            input.y.saturating_sub(height),
            input.width.min(72),
            height,
        );
        frame.render_widget(Clear, area);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(" Model configuration · Preview ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.is_empty() {
            return area;
        }
        // Keep the focused field visible even in a short terminal.
        let start = if inner.height < 10 {
            self.focus * 2 + 1
        } else {
            0
        };
        let labels = ["Model", "Base URL", "API Key"];
        let rows = [
            "Protocol: OpenAI Chat Completions",
            "",
            "",
            "",
            "",
            "",
            "",
            "",
            "Preview only. Changes are not applied.",
            "Tab Next · Shift+Tab Previous · Esc Back",
        ];
        for (row, text) in rows.iter().enumerate().skip(start) {
            let y = inner.y + (row - start) as u16;
            if y >= inner.bottom() {
                break;
            }
            let rect = Rect::new(inner.x, y, inner.width, 1);
            if (1..=6).contains(&row) {
                let field = (row - 1) / 2;
                if row % 2 == 1 {
                    frame.render_widget(
                        Paragraph::new(Line::styled(
                            labels[field],
                            Style::default().fg(if field == self.focus {
                                Color::Cyan
                            } else {
                                Color::Gray
                            }),
                        )),
                        rect,
                    );
                } else {
                    frame.render_widget(&self.fields[field], rect);
                }
            } else {
                frame.render_widget(
                    Paragraph::new(*text).style(Style::default().fg(Color::DarkGray)),
                    rect,
                );
            }
        }
        let cursor_row = self.focus * 2 + 2;
        if cursor_row >= start && cursor_row - start < inner.height as usize {
            let column = self.fields[self.focus].cursor().1 as u16;
            frame.set_cursor_position((
                inner.x + column.min(inner.width.saturating_sub(1)),
                inner.y + (cursor_row - start) as u16,
            ));
        }
        area
    }
}

#[cfg(test)]
mod tests {
    use super::ModelConfigForm;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    #[test]
    fn configuration_is_local_and_masks_pasted_credentials() {
        let mut form = ModelConfigForm::new("example-model".into());
        form.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        form.paste("https://example.com/v1\n");
        form.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        form.paste("secret-example-key");
        form.paste("invalid\nmultiline");
        assert_eq!(form.fields[1].lines(), &["https://example.com/v1"]);
        assert_eq!(form.fields[2].lines(), &["secret-example-key"]);
        let mut terminal = Terminal::new(TestBackend::new(90, 30)).unwrap();
        terminal
            .draw(|frame| {
                let area = form.render(frame, Rect::new(0, 27, 90, 3));
                assert_eq!(area.bottom(), 27);
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("OpenAI Chat Completions"));
        assert!(text.contains("Changes are not applied"));
        assert!(!text.contains("secret-example-key"));
        for (width, height) in [(25, 8), (8, 3)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    form.render(frame, Rect::new(0, height - 1, width, 1));
                })
                .unwrap();
        }
    }
}
