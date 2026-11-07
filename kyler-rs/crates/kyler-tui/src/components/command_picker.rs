use crate::state::State;

use ratatui::{Frame, layout::Rect};

pub struct Command {
    pub name: &'static str,
    pub description: &'static str,
}
pub const COMMANDS: &[Command] = &[
    Command {
        name: "/model",
        description: "Model",
    },
    Command {
        name: "/permissions",
        description: "Permissions",
    },
    Command {
        name: "/mcp",
        description: "MCP",
    },
    Command {
        name: "/skills",
        description: "Skills",
    },
    Command {
        name: "/help",
        description: "Help",
    },
    Command {
        name: "/resume",
        description: "Continue task",
    },
];
pub fn matches(state: &State) -> Vec<&'static Command> {
    if (state.approval_focus && state.pending_approval().is_some())
        || state.approval_details.is_some()
        || state.help
        || state.confirm_exit
        || state.mcp_open
        || state.permissions_open
        || state.model_open
        || state.skills_open
        || state.composer.lines().len() != 1
    {
        return Vec::new();
    }
    let query = &state.composer.lines()[0];
    if !query.starts_with('/') || state.slash_dismissed.as_ref() == Some(query) {
        return Vec::new();
    }
    COMMANDS
        .iter()
        .filter(|command| command.name.starts_with(query.as_str()))
        .collect()
}
pub fn render(frame: &mut Frame, state: &State, input: Rect) -> Rect {
    let commands = matches(state);
    if commands.is_empty() {
        return Rect::default();
    }
    let selected = state.slash_selected.min(commands.len() - 1);
    let lines = commands
        .iter()
        .enumerate()
        .map(|(index, command)| {
            super::panel::selected(
                format!(
                    "{} {:<15} {}",
                    if index == selected { ">" } else { " " },
                    command.name,
                    command.description
                ),
                index == selected,
            )
        })
        .collect();
    super::panel::draw(
        frame,
        input,
        "Commands",
        lines,
        "↑↓ Select · Enter Open · Tab Complete · Esc Close",
        false,
        super::panel::Position::Selected(selected),
    )
    .0
}
