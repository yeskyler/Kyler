//! Keyboard routing, local panels and composer shortcuts.
use super::{cancel, paste_image};
use crate::agent::AgentHandle;
use crate::{
    commands::dispatch_command,
    components::command_picker::COMMANDS,
    effects::{Effects, Sender},
    layout::View,
    state::{Panel, Phase, State},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) fn handle(
    key: KeyEvent,
    state: &mut State,
    effects: &mut Effects,
    client: &AgentHandle,
    send: &Sender,
    view: &View,
) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if let Some(exit) = super::panels::handle(key, state, effects, client, send) {
        return exit;
    }
    if super::completion::handle(key, state, effects, client, send) {
        return false;
    }
    match key.code {
        KeyCode::Char('o') if ctrl => state.toggle_latest_reasoning(),
        KeyCode::Char('r') if ctrl && state.phase == Phase::Ready => {
            if !state.restore_failed_submission() {
                state.notice("Clear the current draft before restoring failed input.");
            }
        }
        KeyCode::Char('v' | 'V') if ctrl => paste_image(None, state, effects, send),
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::ALT) => {
            state.attachments.pop();
        }
        KeyCode::F(1) => state.open_panel(Panel::Help),
        KeyCode::F(10) => state.confirm_exit = true,
        KeyCode::Char('c') if ctrl => {
            if state.cancel_requested {
                return true;
            }
            if matches!(
                state.phase,
                Phase::Running | Phase::Submitting | Phase::Cancelling
            ) || state.run.is_some() && state.phase == Phase::Detached
            {
                cancel(state, effects, client, send);
            } else if !state.composer.lines().join("").is_empty()
                || !state.attachments.is_empty()
                || state.image_busy
            {
                state.confirm_exit = true;
            } else {
                return true;
            }
        }
        KeyCode::Char('n') if ctrl => {
            if matches!(state.phase, Phase::Ready)
                || state.phase == Phase::Detached && state.pending.is_none() && state.run.is_none()
            {
                if state.composer.lines().iter().any(|line| !line.is_empty())
                    || !state.attachments.is_empty()
                    || state.image_busy
                {
                    state.notice("Clear the draft before creating a new session.");
                } else {
                    let generation = state.generation + 1;
                    *state = State::new(None);
                    state.generation = generation;
                    effects.load(client.clone(), None, generation, send.clone());
                }
            } else {
                state.notice("A task is still active. Wait, cancel, or exit the session.");
            }
        }
        KeyCode::F(5) if state.phase == Phase::Detached => {
            if state.run.is_some() {
                state.notice(
                    "The local task is still active. Wait for cancellation or exit the TUI.",
                );
            } else if let (Some(session), Some(pending)) =
                (state.session.clone(), state.pending.clone())
            {
                state.phase = Phase::Submitting;
                effects.start(
                    client.clone(),
                    session,
                    pending,
                    state.generation,
                    send.clone(),
                );
            } else {
                state.phase = Phase::Loading;
                effects.load(
                    client.clone(),
                    state.session.clone(),
                    state.generation,
                    send.clone(),
                );
            }
        }
        KeyCode::PageUp => state.scroll_by(-(view.transcript.height.max(1) as isize)),
        KeyCode::PageDown => state.scroll_by(view.transcript.height.max(1) as isize),
        KeyCode::End if ctrl => {
            state.follow = true;
            state.scroll = state.max_scroll;
        }
        KeyCode::Enter
            if key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
        {
            state.composer.insert_newline();
        }
        KeyCode::Char('j') if ctrl => {
            state.composer.insert_newline();
        }
        KeyCode::Enter => {
            let draft = state.composer.lines().join("\n");
            if let Some(command) = COMMANDS
                .iter()
                .find(|command| command.name == draft.trim() && command.name != "/resume")
            {
                dispatch_command(command.name, state, effects, client, send);
                return false;
            }
            if let Some(submission) = state.begin() {
                if let Some(session) = state.session.clone() {
                    effects.start(
                        client.clone(),
                        session,
                        submission,
                        state.generation,
                        send.clone(),
                    );
                }
            }
        }
        _ => {
            state.slash_dismissed = None;
            state.composer.input(key);
        }
    }

    false
}
