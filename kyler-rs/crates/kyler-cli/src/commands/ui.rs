//! Shared inline progress and prompt styling for installation and configuration.
use std::{io::IsTerminal, time::Duration};

use indicatif::{ProgressBar, ProgressStyle};
use inquire::ui::{Attributes, Color, IndexPrefix, RenderConfig, StyleSheet, Styled};

pub(super) const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Clone, Copy)]
pub(super) enum Tone {
    Active,
    Success,
    Warning,
    Error,
}
fn color_mode(terminal: bool, term: Option<&str>, no_color: bool) -> bool {
    terminal && term != Some("dumb") && !no_color
}
pub(super) fn terminal_controls() -> bool {
    std::io::stderr().is_terminal() && std::env::var("TERM").as_deref() != Ok("dumb")
}
pub(super) fn colors_enabled() -> bool {
    color_mode(
        std::io::stderr().is_terminal(),
        std::env::var("TERM").ok().as_deref(),
        std::env::var_os("NO_COLOR").is_some(),
    )
}
pub(super) fn paint(value: &str, tone: Tone) -> String {
    if !colors_enabled() {
        return value.into();
    }
    let code = match tone {
        Tone::Active => "1;36",
        Tone::Success => "32",
        Tone::Warning => "33",
        Tone::Error => "31",
    };
    format!("\x1b[{code}m{value}\x1b[0m")
}
pub(super) fn field(label: &str, value: &str) {
    eprintln!("{label}:{value}");
}
pub(super) fn warning(message: &str) {
    eprintln!("{} {message}", paint("□", Tone::Warning));
}
pub(super) fn error(message: &str) {
    eprintln!("{} {message}", paint("■", Tone::Error));
}

pub(super) struct Progress {
    bar: ProgressBar,
    active: Option<&'static str>,
    terminal: bool,
}
impl Progress {
    pub fn new() -> Self {
        // Keep the first frame on the current line, without a reserved header row.
        if terminal_controls() {
            eprint!("\r");
        }
        Self {
            bar: ProgressBar::hidden(),
            active: None,
            terminal: terminal_controls(),
        }
    }
    pub fn begin(&mut self, label: &'static str) {
        self.bar.finish_and_clear();
        self.active = Some(label);
        self.bar = ProgressBar::new_spinner();
        self.bar.set_style(
            ProgressStyle::with_template(if colors_enabled() {
                "{spinner:.cyan} {msg}"
            } else {
                "{spinner} {msg}"
            })
            .unwrap()
            .tick_strings(&FRAMES),
        );
        self.bar.set_message(label);
        if self.terminal {
            self.bar.tick();
            self.bar.enable_steady_tick(Duration::from_millis(100));
        }
    }
    pub fn detail(&self, detail: &str) {
        if let Some(label) = self.active {
            self.bar.set_message(format!("{label} ({detail})"));
        }
    }
    pub fn pause(&self) {
        self.bar.disable_steady_tick();
        self.bar.finish_and_clear();
    }
    pub fn dismiss(&mut self) {
        self.bar.finish_and_clear();
        self.active = None;
    }
    pub fn skipped(&mut self, reason: &str) {
        self.bar.finish_and_clear();
        if let Some(label) = self.active.take() {
            eprintln!("{} {label}:{reason}", paint("□", Tone::Warning));
        }
    }
    pub fn complete(&mut self) {
        self.bar.disable_steady_tick();
        self.bar.finish_and_clear();
        if let Some(label) = self.active.take() {
            eprintln!("{} {label}", paint("■", Tone::Success));
        }
    }
    pub fn fail(&mut self) {
        self.bar.disable_steady_tick();
        self.bar.finish_and_clear();
        if let Some(label) = self.active.take() {
            eprintln!("{} {label}", paint("▧", Tone::Error));
        }
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        self.bar.finish_and_clear();
    }
}

pub(super) fn theme() -> RenderConfig<'static> {
    theme_for(colors_enabled())
}
pub(super) fn model_list_theme() -> RenderConfig<'static> {
    let mut config = theme();
    config.option_index_prefix = IndexPrefix::Simple;
    config.scroll_up_prefix = Styled::new("");
    config.scroll_down_prefix = Styled::new("");
    config
}

fn theme_for(colored: bool) -> RenderConfig<'static> {
    let mut config = RenderConfig::empty();
    config.prompt_prefix = Styled::new("▣");
    config.answered_prompt_prefix = Styled::new("■");
    config.highlighted_option_prefix = Styled::new("");
    config.scroll_up_prefix = Styled::new("");
    config.scroll_down_prefix = Styled::new("");
    config.unhighlighted_option_prefix = Styled::new("");
    config.canceled_prompt_indicator = Styled::new("Back");
    config.error_message.prefix = Styled::new("■");
    if colored {
        config.prompt_prefix = config
            .prompt_prefix
            .with_fg(Color::DarkCyan)
            .with_attr(Attributes::BOLD);
        config.answered_prompt_prefix = config.answered_prompt_prefix.with_fg(Color::DarkGreen);
        config.highlighted_option_prefix = config
            .highlighted_option_prefix
            .with_fg(Color::DarkCyan)
            .with_attr(Attributes::BOLD);
        config.selected_option = Some(
            StyleSheet::empty()
                .with_fg(Color::DarkCyan)
                .with_attr(Attributes::BOLD),
        );
        config.canceled_prompt_indicator =
            config.canceled_prompt_indicator.with_fg(Color::DarkYellow);
        config.error_message.prefix = config.error_message.prefix.with_fg(Color::DarkRed);
        config.error_message.message = StyleSheet::empty().with_fg(Color::DarkRed);
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn themes_inherit_terminal_background_and_default_text() {
        for colored in [true, false] {
            let config = theme_for(colored);
            assert!(config.prompt.bg.is_none() && config.prompt.fg.is_none());
            for style in [
                config.prompt_prefix.style,
                config.answered_prompt_prefix.style,
                config.highlighted_option_prefix.style,
                config.option,
                config.answer,
                config.help_message,
                config.placeholder,
                config.error_message.message,
                config.selected_option.unwrap_or_default(),
            ] {
                assert!(style.bg.is_none());
                if !colored {
                    assert!(style.is_empty());
                }
            }
        }
        assert!(!color_mode(false, Some("xterm"), false));
        assert!(!color_mode(true, Some("dumb"), false));
        assert!(!color_mode(true, Some("xterm"), true));
        assert!(color_mode(true, Some("xterm"), false));
    }
}
