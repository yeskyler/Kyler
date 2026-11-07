//! Terminal input, keyboard shortcuts, panel navigation and attachment paste.
mod completion;
mod keyboard;
mod mouse;
mod panels;

use crate::{
    components::composer,
    effects::{Effects, Sender},
    layout::View,
    state::{Phase, State},
};

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use crossterm::event::{Event as Input, KeyEventKind};

use kyler_client::Client;
use kyler_protocol::api::attachments::MAX_ATTACHMENTS;

pub(crate) fn cancel(state: &mut State, effects: &mut Effects, client: &Client, send: &Sender) {
    state.cancel_requested = true;
    state.phase = Phase::Cancelling;
    state.dirty = true;
    state.cancel_deadline =
        Some(Instant::now() + Duration::from_secs(client.settings().cancel_timeout_seconds));
    if let Some(run) = state.run.clone() {
        effects.cancel(client.clone(), run, state.generation, send.clone());
    }
}
pub(crate) fn handle_input(
    event: Input,
    state: &mut State,
    effects: &mut Effects,
    client: &Client,
    send: &Sender,
    view: &View,
) -> bool {
    state.dirty = true;
    match event {
        Input::Resize(..) => {}
        Input::Paste(text) if state.questions.pending().is_some() => {
            crate::questions::paste(&text, state)
        }
        Input::Paste(text) if state.model_open && state.model_form.is_some() => {
            if let Some(form) = state.model_form.as_mut() {
                form.paste(&text);
            }
        }
        Input::Paste(text)
            if !state.help
                && !state.confirm_exit
                && !state.mcp_open
                && !state.skills_open
                && !state.model_open
                && !state.permissions_open
                && state.approval_details.is_none() =>
        {
            state.slash_dismissed = None;
            if let Some(path) = crate::attachments::pasted_path(&text) {
                paste_image(Some(path), state, effects, send);
            } else {
                state.composer.insert_str(composer::clean(&text));
            }
        }
        Input::Mouse(mouse) => return mouse::handle(mouse, state, view),
        Input::Key(key) if key.kind != KeyEventKind::Release => {
            return keyboard::handle(key, state, effects, client, send, view);
        }

        _ => {}
    }
    false
}

fn scroll_panel(state: &mut State, delta: isize) {
    let offset = |n: usize| n.saturating_add_signed(delta);
    if state.approval_details.is_some() {
        state.approval_scroll = offset(state.approval_scroll);
    } else if state.help || state.confirm_exit {
        state.help_scroll = offset(state.help_scroll);
    } else if state.model_open && state.model_form.is_some() {
        // Scrolling does not change the underlying model selection while editing.
    } else if state.model_open {
        if state.model_details {
            state.model_scroll = offset(state.model_scroll);
        } else {
            state.model_selected = offset(state.model_selected).min(
                state
                    .model_catalog
                    .as_ref()
                    .map(|c| c.models.len().saturating_sub(1))
                    .unwrap_or(0),
            );
        }
    } else if state.permissions_open {
        if state.permissions_details || state.permissions_confirm {
            state.permissions_scroll =
                offset(state.permissions_scroll as usize).min(u16::MAX as usize) as u16;
        } else {
            state.permissions_selected = offset(state.permissions_selected).min(3);
        }
    } else if state.mcp_open {
        if state.mcp_details {
            state.mcp_scroll = offset(state.mcp_scroll);
        } else {
            state.mcp_selected = offset(state.mcp_selected).min(state.mcp.len().saturating_sub(1));
        }
    } else if state.skills_open {
        if state.skill_details.is_some() {
            state.skills_scroll = offset(state.skills_scroll);
        } else {
            state.skills_selected = offset(state.skills_selected).min(state.skills.len() + 1);
        }
    } else if !crate::components::skill_picker::candidates(state).is_empty() {
        state.skill_selected = offset(state.skill_selected).min(
            crate::components::skill_picker::candidates(state)
                .len()
                .saturating_sub(1),
        );
    } else {
        state.slash_selected = offset(state.slash_selected).min(
            crate::components::command_picker::matches(state)
                .len()
                .saturating_sub(1),
        );
    }
}

pub(crate) fn paste_image(
    path: Option<PathBuf>,
    state: &mut State,
    effects: &mut Effects,
    send: &Sender,
) {
    if state.image_busy {
        return;
    }
    if state.attachments.len() >= MAX_ATTACHMENTS {
        state.notice("At most eight attachments are allowed per message.");
        return;
    }
    state.image_busy = true;
    effects.paste_image(path, state.generation, state.draft_epoch, send.clone());
}
