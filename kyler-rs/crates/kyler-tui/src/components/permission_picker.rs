//! Compact permission picker anchored to the composer footer.
use super::{
    composer::clean,
    panel::{self, Position},
};
use crate::{
    effects::{Effects, Sender},
    state::{Entry, Panel, State},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
};

use kyler_client::Client;
use kyler_protocol::api::permissions::{ApprovalDecision, PermissionMode, PermissionSelection};

pub fn open(state: &mut State) {
    state.open_panel(Panel::Permissions);
    state.permissions_scroll = 0;
    state.permissions_confirm = false;
    if let Some(policy) = &state.permissions {
        state.permissions_selected = PermissionMode::ALL
            .iter()
            .position(|m| *m == policy.selected.mode.preset())
            .unwrap_or(0);
    }
}
pub fn handle_key(
    key: KeyEvent,
    state: &mut State,
    effects: &mut Effects,
    client: &Client,
    send: &Sender,
) {
    match key.code {
        KeyCode::Esc if state.permissions_confirm => {
            state.permissions_confirm = false;
            state.permissions_scroll = 0;
        }
        KeyCode::Esc if state.permissions_details => {
            state.permissions_details = false;
            state.permissions_scroll = 0;
        }
        KeyCode::Esc => state.permissions_open = false,
        KeyCode::Char('i') if !state.permissions_confirm => {
            state.permissions_details = !state.permissions_details;
            state.permissions_scroll = 0;
        }
        KeyCode::Up if state.permissions_details || state.permissions_confirm => {
            state.permissions_scroll = state.permissions_scroll.saturating_sub(1)
        }
        KeyCode::Down if state.permissions_details || state.permissions_confirm => {
            state.permissions_scroll = state.permissions_scroll.saturating_add(1)
        }
        KeyCode::PageUp if !state.permissions_details && !state.permissions_confirm => {
            state.permissions_selected = 0
        }
        KeyCode::PageDown if !state.permissions_details && !state.permissions_confirm => {
            state.permissions_selected = 2
        }
        KeyCode::PageUp => state.permissions_scroll = state.permissions_scroll.saturating_sub(4),
        KeyCode::PageDown => state.permissions_scroll = state.permissions_scroll.saturating_add(4),
        KeyCode::Up if !state.permissions_confirm => {
            state.permissions_scroll = 0;
            state.permissions_selected = state.permissions_selected.saturating_sub(1)
        }
        KeyCode::Down if !state.permissions_confirm => {
            state.permissions_scroll = 0;
            state.permissions_selected = (state.permissions_selected + 1).min(2)
        }
        KeyCode::Enter if !state.permissions_busy => {
            let mode = PermissionMode::ALL[state.permissions_selected.min(2)];
            if state.permissions.as_ref().is_some_and(|state| {
                !state
                    .options
                    .iter()
                    .any(|option| option.mode == mode && option.available)
            }) {
                state.permissions_error =
                    Some("This preset is unavailable under the server policy.".into());
                return;
            }
            if state.permissions.is_none() {
                state.permissions_error = Some("Wait for the permission state to load.".into());
                return;
            }
            if mode == PermissionMode::FullAccess && !state.permissions_confirm {
                state.permissions_confirm = true;
                return;
            }
            if let Some(session) = state.session.clone() {
                state.permissions_epoch += 1;
                state.permissions_busy = true;
                state.permissions_error = None;
                effects.select_permissions(
                    client.clone(),
                    session,
                    PermissionSelection {
                        approval: None,
                        mode: Some(mode),
                        confirm_full_access: state.permissions_confirm,
                    },
                    state.generation,
                    state.permissions_epoch,
                    send.clone(),
                );
            }
        }
        _ => {}
    }
}
pub fn approval_key(
    key: KeyEvent,
    state: &mut State,
    effects: &mut Effects,
    client: &Client,
    send: &Sender,
) -> bool {
    let Some(index) = state.pending_approval() else {
        return false;
    };
    if !state.approval_focus {
        return false;
    }
    if key.code == KeyCode::Esc {
        state.approval_focus = false;
        return true;
    }
    if key.code == KeyCode::Char('e') {
        let Entry::Approval { record, .. } = &state.entries[index] else {
            return false;
        };
        let id = record.id.clone();
        state.open_panel(Panel::Approval);
        state.approval_details = Some(id);
        state.approval_scroll = 0;
        return true;
    }
    if state.approval_busy {
        return true;
    }
    let Entry::Approval { record, choice, .. } = &mut state.entries[index] else {
        return false;
    };
    let plain = key.modifiers.difference(KeyModifiers::SHIFT).is_empty();
    match key.code {
        KeyCode::Char('y' | 'Y' | 'n' | 'N') | KeyCode::Enter if plain => {
            match key.code {
                KeyCode::Char('y' | 'Y') => *choice = 0,
                KeyCode::Char('n' | 'N') => *choice = 2,
                _ => {}
            }
            state.approval_busy = true;
            state.approval_error = None;
            let decision = if *choice == 0 {
                ApprovalDecision::AllowOnce
            } else {
                ApprovalDecision::Deny
            };
            effects.decide_approval(
                client.clone(),
                *record.clone(),
                decision,
                state.generation,
                send.clone(),
            );
        }
        KeyCode::Left
        | KeyCode::Right
        | KeyCode::Up
        | KeyCode::Down
        | KeyCode::Tab
        | KeyCode::BackTab
            if plain =>
        {
            *choice = if *choice == 0 { 2 } else { 0 };
        }
        KeyCode::PageUp => state.scroll_by(-8),
        KeyCode::PageDown => state.scroll_by(8),
        KeyCode::Char(_) | KeyCode::Enter => return true,
        _ => return false,
    }
    true
}
pub fn render(frame: &mut Frame, state: &mut State, input: Rect) -> Rect {
    let details = state.permissions_details || state.permissions_confirm;
    let mut lines = Vec::new();
    if details {
        if state.permissions_confirm {
            lines.push(Line::styled(
                "Allow unrestricted file and network access without approval prompts?",
                Style::default().fg(Color::Yellow),
            ));
        }
        if let Some(policy) = &state.permissions {
            lines.push(Line::from(clean(&policy.ceiling)));
            lines.push(Line::from(format!(
                "Sandbox: {}",
                policy
                    .sandbox
                    .map(|mode| mode.label())
                    .unwrap_or("unavailable")
            )));
            if let Some(active) = &policy.active {
                lines.push(Line::from(format!(
                    "Running: {}",
                    active.mode.preset().label()
                )));
            }
            lines.push(Line::from("Changes apply to subsequent tool calls."));
        }
    } else {
        for (index, policy) in PermissionMode::ALL.iter().enumerate() {
            let checked = state
                .permissions
                .as_ref()
                .is_some_and(|s| s.selected.mode.preset() == *policy);
            lines.push(panel::selected(
                format!(
                    "{} {}{}",
                    if index == state.permissions_selected {
                        ">"
                    } else {
                        " "
                    },
                    policy.label(),
                    if state.permissions.as_ref().is_some_and(|s| !s
                        .options
                        .iter()
                        .any(|option| option.mode == *policy && option.available))
                    {
                        " · unavailable"
                    } else if checked {
                        " ✓"
                    } else {
                        ""
                    }
                ),
                index == state.permissions_selected,
            ));
        }
    }
    if state.permissions_busy {
        lines.push(Line::from("Saving…"));
    }
    if let Some(error) = &state.permissions_error {
        lines.push(Line::styled(clean(error), Style::default().fg(Color::Red)));
    }
    let (area, scroll) = panel::draw(
        frame,
        input,
        if state.permissions_confirm {
            "Permissions › Confirm Full access"
        } else if details {
            "Permissions › Details"
        } else {
            "Permissions"
        },
        lines,
        if state.permissions_confirm {
            "↑↓ Scroll · Enter Confirm · Esc Back"
        } else if details {
            "↑↓ Scroll · Esc Back"
        } else {
            "Enter Apply · i Details"
        },
        details,
        if details {
            Position::Scroll(state.permissions_scroll as usize)
        } else {
            Position::Selected(state.permissions_selected)
        },
    );
    if details {
        state.permissions_scroll = scroll.min(u16::MAX as usize) as u16;
    }
    area
}
pub fn render_approval(frame: &mut Frame, state: &mut State, input: Rect) -> Rect {
    let mut lines = Vec::new();
    if let Some(Entry::Approval { record, .. }) = state.entries.iter().find(|entry| matches!(entry, Entry::Approval { record, .. } if Some(&record.id) == state.approval_details.as_ref())) {
        lines.push(Line::from(format!("{} · {:?}", clean(&record.tool), record.status)));
        if let Some(cwd) = &record.working_directory { lines.push(Line::from(format!("Directory: {}", clean(cwd)))); }
        lines.push(Line::from(format!("{} · Session {} · {}", clean(&record.operation), clean(&record.origin_session_id), record.policy.mode.preset().label())));
        lines.extend(super::approval::details(record).into_iter().map(|line| Line::from(clean(&line))));
        lines.push(Line::from(clean(&record.reason)));
    }
    let (area, scroll) = panel::draw(
        frame,
        input,
        "Approval › Details",
        lines,
        "↑↓ Scroll · Esc Back",
        true,
        Position::Scroll(state.approval_scroll),
    );
    state.approval_scroll = scroll;
    area
}
