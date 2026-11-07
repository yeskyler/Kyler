//! Focused panel keyboard routing and global panel shortcuts.
use crate::{
    effects::{Effects, Sender},
    state::{Entry, Panel, State},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use kyler_client::Client;
use kyler_protocol::api::mcp::McpAction;

pub(super) fn handle(
    key: KeyEvent,
    state: &mut State,
    effects: &mut Effects,
    client: &Client,
    send: &Sender,
) -> Option<bool> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if state.confirm_exit {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => return Some(true),
            KeyCode::Esc | KeyCode::Char('n') => state.confirm_exit = false,
            _ => {}
        }
        return Some(false);
    }
    if crate::questions::keyboard(key, state, effects, client, send) {
        return Some(false);
    }
    if state.tool_details.is_some() {
        match key.code {
            KeyCode::Esc => state.tool_details = None,
            KeyCode::Up => state.tool_scroll = state.tool_scroll.saturating_sub(1),
            KeyCode::Down => state.tool_scroll += 1,
            KeyCode::PageDown => state.tool_scroll += 5,
            KeyCode::PageUp => state.tool_scroll = state.tool_scroll.saturating_sub(5),
            _ => {}
        }
        return Some(false);
    }
    if key.code == KeyCode::F(3) {
        if let (Some(session), Some(call)) = (
            state.session.clone(),
            state.entries.iter().rev().find_map(|entry| match entry {
                Entry::Tool { id, .. } => Some(id.clone()),
                _ => None,
            }),
        ) {
            effects.tool_details(
                client.clone(),
                session,
                call,
                state.generation,
                send.clone(),
            );
        } else {
            state.notice("No tool calls to inspect.");
        }
        return Some(false);
    }
    if state.approval_details.is_some() {
        match key.code {
            KeyCode::Esc => state.approval_details = None,
            KeyCode::Up => state.approval_scroll = state.approval_scroll.saturating_sub(1),
            KeyCode::Down => state.approval_scroll += 1,
            KeyCode::PageUp => state.approval_scroll = state.approval_scroll.saturating_sub(5),
            KeyCode::PageDown => state.approval_scroll += 5,
            _ => {}
        }
        return Some(false);
    }
    if key.code == KeyCode::F(1) && !state.help {
        state.open_panel(Panel::Help);
        return Some(false);
    }
    if state.permissions_open {
        crate::components::permission_picker::handle_key(key, state, effects, client, send);
        state.back_to_commands();
        return Some(false);
    }
    if key.code == KeyCode::Char('p') && ctrl {
        crate::components::permission_picker::open(state);
        return Some(false);
    }
    if key.code == KeyCode::F(2) && state.pending_approval().is_some() {
        state.open_panel(Panel::Approval);
        state.approval_focus = true;
        state.follow = true;
        return Some(false);
    }
    if !ctrl
        && !state.panel_open()
        && crate::components::permission_picker::approval_key(key, state, effects, client, send)
    {
        return Some(false);
    }
    if state.model_open {
        crate::components::model_picker::handle_key(key, state, effects, client, send);
        state.back_to_commands();
        return Some(false);
    }
    if state.skills_open {
        crate::components::skills_panel::handle_key(key, state, effects, client, send);
        state.back_to_commands();
        return Some(false);
    }
    if state.mcp_open {
        match key.code {
            KeyCode::Esc if state.mcp_details => {
                state.mcp_details = false;
                state.mcp_scroll = 0;
            }
            KeyCode::Esc => state.mcp_open = false,
            KeyCode::Enter => {
                state.mcp_details = !state.mcp_details;
                state.mcp_scroll = 0;
            }
            KeyCode::Up if state.mcp_details => {
                state.mcp_scroll = state.mcp_scroll.saturating_sub(1)
            }
            KeyCode::Down if state.mcp_details => state.mcp_scroll += 1,
            KeyCode::PageUp if state.mcp_details => {
                state.mcp_scroll = state.mcp_scroll.saturating_sub(8)
            }
            KeyCode::PageDown if state.mcp_details => state.mcp_scroll += 8,
            KeyCode::PageUp => state.mcp_selected = state.mcp_selected.saturating_sub(5),
            KeyCode::PageDown => {
                state.mcp_selected = (state.mcp_selected + 5).min(state.mcp.len().saturating_sub(1))
            }
            KeyCode::Up => state.mcp_selected = state.mcp_selected.saturating_sub(1),
            KeyCode::Down => {
                state.mcp_selected = (state.mcp_selected + 1).min(state.mcp.len().saturating_sub(1))
            }
            KeyCode::Char(c @ ('c' | 'd' | 'r')) if !ctrl => {
                if let (Some(session), Some(server)) =
                    (state.session.clone(), state.mcp.get(state.mcp_selected))
                {
                    let action = match c {
                        'c' => McpAction::Connect,
                        'd' => McpAction::Disconnect,
                        _ => McpAction::Reconnect,
                    };
                    effects.mcp_action(
                        client.clone(),
                        session,
                        server.id.clone(),
                        action,
                        state.generation,
                        send.clone(),
                    );
                }
            }
            _ => {}
        }
        state.back_to_commands();
        return Some(false);
    }
    if state.help {
        match key.code {
            KeyCode::Esc | KeyCode::F(1) => {
                state.help = false;
                state.back_to_commands();
            }
            KeyCode::Up => state.help_scroll = state.help_scroll.saturating_sub(1),
            KeyCode::Down => state.help_scroll += 1,
            KeyCode::PageUp => state.help_scroll = state.help_scroll.saturating_sub(5),
            KeyCode::PageDown => state.help_scroll += 5,
            _ => {}
        }
        return Some(false);
    }

    None
}
