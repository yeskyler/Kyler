use super::panel::{self, Position};
use crate::{components::composer::clean, state::State};

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
};

use kyler_protocol::api::mcp::McpStatus;

pub fn render(frame: &mut Frame, state: &mut State, input: Rect) -> Rect {
    let mut lines = Vec::new();
    let mut selected_row = 0;
    if state.mcp_details {
        if let Some(server) = state.mcp.get(state.mcp_selected) {
            lines.push(Line::from(format!(
                "{} · {}",
                clean(&server.id),
                clean(&server.transport)
            )));
            if let Some(error) = &server.error {
                lines.push(Line::styled(clean(error), Style::default().fg(Color::Red)));
            }
            if server.tools.is_empty() {
                lines.push(Line::from("No tools"));
            }
            for tool in &server.tools {
                lines.push(Line::styled(
                    clean(&tool.name),
                    Style::default().fg(Color::Cyan),
                ));
                lines.push(Line::from(clean(&tool.description)));
            }
        }
    } else {
        for (index, server) in state.mcp.iter().enumerate() {
            if index == state.mcp_selected {
                selected_row = lines.len();
            }
            let label = match server.status {
                McpStatus::Connected => "Connected",
                McpStatus::Connecting => "Connecting",
                McpStatus::Disconnected => "Disconnected",
                McpStatus::Failed => "Failed",
            };
            lines.push(panel::selected(
                format!(
                    "{} {} · {label}",
                    if index == state.mcp_selected {
                        ">"
                    } else {
                        " "
                    },
                    clean(&server.id)
                ),
                index == state.mcp_selected,
            ));
        }
        if state.mcp.is_empty() {
            lines.push(Line::from("No MCP servers"));
        }
        if let Some(error) = &state.mcp_error {
            lines.push(Line::styled(clean(error), Style::default().fg(Color::Red)));
        }
    }
    let (area, scroll) = panel::draw(
        frame,
        input,
        if state.mcp_details {
            "MCP › Server details"
        } else {
            "MCP"
        },
        lines,
        if state.mcp_details {
            "↑↓ Scroll · Esc Back"
        } else {
            "Enter Details · c Connect · d Disconnect · r Retry"
        },
        state.mcp_details,
        if state.mcp_details {
            Position::Scroll(state.mcp_scroll)
        } else {
            Position::Selected(selected_row)
        },
    );
    if state.mcp_details {
        state.mcp_scroll = scroll;
    }
    area
}
