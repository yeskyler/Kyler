//! Syntax highlighting is local and produces styled text, never executable terminal escapes.
use std::sync::OnceLock;

use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, ThemeSet},
    parsing::SyntaxSet,
};

pub const CODE_BACKGROUND: Color = Color::Rgb(35, 39, 47);

pub fn highlight(source: &str, language: &str) -> Vec<Vec<Span<'static>>> {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    let syntaxes = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
    let themes = THEMES.get_or_init(ThemeSet::load_defaults);
    let token = language.split_whitespace().next().unwrap_or("");
    let token = match token {
        "bash" | "shell" => "sh",
        "python" => "py",
        "javascript" => "js",
        "typescript" => "ts",
        other => other,
    };
    let syntax = syntaxes
        .find_syntax_by_token(token)
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, &themes.themes["base16-ocean.dark"]);
    source
        .strip_suffix('\n')
        .unwrap_or(source)
        .split('\n')
        .map(|line| {
            if line.len() > 16384 {
                return vec![Span::styled(
                    line.to_owned(),
                    Style::default().fg(Color::Gray),
                )];
            }
            let input = format!("{line}\n");
            match highlighter.highlight_line(&input, syntaxes) {
                Ok(ranges) => ranges
                    .into_iter()
                    .filter_map(|(style, text)| {
                        let text = text.strip_suffix('\n').unwrap_or(text);
                        if text.is_empty() {
                            return None;
                        }
                        let mut terminal = Style::default().fg(Color::Rgb(
                            style.foreground.r,
                            style.foreground.g,
                            style.foreground.b,
                        ));
                        for (flag, modifier) in [
                            (FontStyle::BOLD, Modifier::BOLD),
                            (FontStyle::ITALIC, Modifier::ITALIC),
                            (FontStyle::UNDERLINE, Modifier::UNDERLINED),
                        ] {
                            if style.font_style.contains(flag) {
                                terminal = terminal.add_modifier(modifier);
                            }
                        }
                        Some(Span::styled(text.to_owned(), terminal))
                    })
                    .collect(),
                Err(_) => vec![Span::styled(
                    line.to_owned(),
                    Style::default().fg(Color::Gray),
                )],
            }
        })
        .collect()
}
