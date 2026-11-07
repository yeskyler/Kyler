use std::io::{self, IsTerminal};

use anyhow::{Result, ensure};
use crossterm::{
    cursor::SetCursorStyle,
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
};

pub struct TerminalGuard;
impl TerminalGuard {
    pub fn enter() -> Result<(Self, ratatui::DefaultTerminal)> {
        ensure!(
            io::stdin().is_terminal() && io::stdout().is_terminal(),
            "TUI requires an interactive terminal; use `kyler chat` or `kyler run` for pipes."
        );
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
        let guard = Self;
        let terminal = ratatui::try_init()?;
        execute!(
            io::stdout(),
            EnableBracketedPaste,
            EnableMouseCapture,
            SetCursorStyle::BlinkingBar
        )?;
        Ok((guard, terminal))
    }
}
fn restore() {
    let _ = execute!(
        io::stdout(),
        DisableBracketedPaste,
        DisableMouseCapture,
        SetCursorStyle::DefaultUserShape
    );
    ratatui::restore();
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}
