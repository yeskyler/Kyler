//! Shared Clack theme and progress rendering for installation and setup.
use std::{io::IsTerminal, time::Duration};

use cliclack::{Theme, ThemeState};
use console::Style;
use indicatif::{ProgressBar, ProgressStyle};

pub(super) const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(super) fn terminal_controls() -> bool {
    std::io::stderr().is_terminal() && std::env::var("TERM").as_deref() != Ok("dumb")
}
fn colors_enabled() -> bool {
    terminal_controls() && std::env::var_os("NO_COLOR").is_none()
}
pub(super) fn initialize() {
    console::set_colors_enabled(colors_enabled());
    console::set_colors_enabled_stderr(colors_enabled());
    cliclack::set_theme(KylerTheme);
}
struct KylerTheme;
impl Theme for KylerTheme {
    fn bar_color(&self, state: &ThemeState) -> Style {
        match state {
            ThemeState::Active => Style::new().for_stderr().cyan(),
            ThemeState::Cancel => Style::new().for_stderr().red(),
            ThemeState::Error(_) => Style::new().for_stderr().yellow(),
            ThemeState::Submit => Style::new().for_stderr().dim(),
        }
    }
    fn input_style(&self, state: &ThemeState) -> Style {
        match state {
            ThemeState::Cancel => Style::new().for_stderr().dim(),
            _ => Style::new().for_stderr(),
        }
    }
    fn radio_symbol(&self, state: &ThemeState, selected: bool) -> String {
        match state {
            ThemeState::Active | ThemeState::Error(_) if selected => {
                Style::new().for_stderr().cyan().apply_to("●").to_string()
            }
            ThemeState::Active | ThemeState::Error(_) => {
                Style::new().for_stderr().dim().apply_to("○").to_string()
            }
            _ => String::new(),
        }
    }
    fn radio_item(&self, state: &ThemeState, selected: bool, label: &str, hint: &str) -> String {
        if matches!(state, ThemeState::Submit | ThemeState::Cancel) {
            return if selected {
                label.into()
            } else {
                String::new()
            };
        }
        let label = if selected {
            Style::new()
                .for_stderr()
                .cyan()
                .bold()
                .apply_to(label)
                .to_string()
        } else {
            label.into()
        };
        let hint = if selected && !hint.is_empty() {
            format!(
                " {}",
                Style::new()
                    .for_stderr()
                    .dim()
                    .apply_to(format!("({hint})"))
            )
        } else {
            String::new()
        };
        format!("{} {label}{hint}", self.radio_symbol(state, selected))
    }
    fn format_scroll_indicator(&self, _: &ThemeState, _: bool, _: bool) -> String {
        String::new()
    }
}
pub(super) fn credential_frame(label: &str, hint: &str) -> String {
    format!(
        "{}│  {hint}\n{}",
        KylerTheme.format_header(&ThemeState::Active, label),
        KylerTheme.format_footer(&ThemeState::Active)
    )
}
pub(super) fn answer(label: &str, value: &str) {
    eprint!(
        "{}│  {value}\n│\n",
        KylerTheme.format_header(&ThemeState::Submit, label)
    );
}
pub(super) fn field(label: &str, value: &str) {
    if terminal_controls() {
        let _ = cliclack::log::remark(format!("{label}: {value}"));
    } else {
        eprintln!("{label}: {value}");
    }
}
pub(super) fn note(title: &str, content: &str) -> std::io::Result<()> {
    initialize();
    if terminal_controls() {
        cliclack::note(title, content)
    } else {
        eprintln!("{title}\n{content}");
        Ok(())
    }
}
pub(super) fn connection(content: &str) -> std::io::Result<()> {
    initialize();
    if terminal_controls() {
        cliclack::outro_note("Connection", content)
    } else {
        eprintln!("{content}");
        Ok(())
    }
}
pub(super) fn warning(message: &str) {
    if terminal_controls() {
        let _ = cliclack::log::warning(message);
    } else {
        eprintln!("Warning: {message}");
    }
}
pub(super) fn error(message: &str) {
    if terminal_controls() {
        let _ = cliclack::log::error(message);
    } else {
        eprintln!("Error: {message}");
    }
}
pub(super) fn step(label: &str, state: ThemeState) {
    initialize();
    if terminal_controls() {
        eprint!("{}", KylerTheme.format_header(&state, label));
        eprintln!("{}", KylerTheme.bar_color(&state).apply_to("│"));
    } else {
        eprintln!("{}  {label}", KylerTheme.state_symbol(&state));
    }
}

pub(super) struct Progress {
    bar: ProgressBar,
    active: Option<&'static str>,
    terminal: bool,
}
impl Progress {
    pub fn new() -> Self {
        initialize();
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
                "{spinner:.cyan}  {msg}"
            } else {
                "{spinner}  {msg}"
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
            self.bar.set_message(format!("{label} · {detail}"));
        }
    }
    pub fn pause(&self) {
        self.bar.disable_steady_tick();
        self.bar.finish_and_clear();
    }
    pub fn dismiss(&mut self) {
        self.pause();
        self.active = None;
    }
    pub fn skipped(&mut self, reason: &str) {
        self.pause();
        if let Some(label) = self.active.take() {
            warning(&format!("{label}: {reason}"));
        }
    }
    pub fn complete(&mut self) {
        self.pause();
        if let Some(label) = self.active.take() {
            step(label, ThemeState::Submit);
        }
    }
    pub fn fail(&mut self) {
        self.pause();
        if let Some(label) = self.active.take() {
            step(label, ThemeState::Error("Failed".into()));
        }
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        self.bar.finish_and_clear();
    }
}
