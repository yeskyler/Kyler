//! Slash-command and attachment completion before composer input.
use super::paste_image;
use crate::{
    commands::dispatch_command,
    components::composer,
    effects::{Effects, Sender},
    state::State,
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use kyler_client::Client;

pub(super) fn handle(
    key: KeyEvent,
    state: &mut State,
    effects: &mut Effects,
    client: &Client,
    send: &Sender,
) -> bool {
    let skills = crate::components::skill_picker::candidates(state)
        .into_iter()
        .map(|skill| skill.name.clone())
        .collect::<Vec<_>>();
    if !skills.is_empty() {
        state.skill_selected = state.skill_selected.min(skills.len() - 1);
        match key.code {
            KeyCode::Up => {
                state.skill_selected = state.skill_selected.saturating_sub(1);
                return true;
            }
            KeyCode::Down => {
                state.skill_selected = (state.skill_selected + 1).min(skills.len() - 1);
                return true;
            }
            KeyCode::Esc => {
                state.skill_dismissed = Some(state.composer.lines().join("\n"));
                return true;
            }
            KeyCode::Tab | KeyCode::Enter if key.modifiers == KeyModifiers::NONE => {
                let name = skills[state.skill_selected].clone();
                let prefix = crate::components::skill_picker::query(state).unwrap().0;
                state.composer = composer::new();
                state.composer.insert_str(format!("{prefix}${name} "));
                state.skill_selected = 0;
                return true;
            }
            _ => state.skill_selected = 0,
        }
    }
    let items = crate::components::attachment_picker::candidates(state);
    if !items.is_empty() {
        state.at_selected = state.at_selected.min(items.len() - 1);
        match key.code {
            KeyCode::Up => {
                state.at_selected = state.at_selected.saturating_sub(1);
                return true;
            }
            KeyCode::Down => {
                state.at_selected = (state.at_selected + 1).min(items.len() - 1);
                return true;
            }
            KeyCode::Esc => {
                if let Some((prefix, _)) = crate::components::attachment_picker::query(state) {
                    state.composer = composer::new();
                    state.composer.insert_str(prefix);
                }
                return true;
            }
            KeyCode::Enter | KeyCode::Tab if key.modifiers == KeyModifiers::NONE => {
                let item = &items[state.at_selected];
                let prefix = crate::components::attachment_picker::query(state)
                    .unwrap()
                    .0;
                state.composer = composer::new();
                state.composer.insert_str(&prefix);
                if item.directory {
                    state
                        .composer
                        .insert_str(format!("@{}/", item.path.display()));
                } else {
                    paste_image(Some(item.path.clone()), state, effects, send);
                }
                state.at_selected = 0;
                return true;
            }
            _ => {}
        }
    }
    let commands = crate::components::command_picker::matches(state);
    if !commands.is_empty() {
        state.slash_selected = state.slash_selected.min(commands.len() - 1);
        match key.code {
            KeyCode::Up => {
                state.slash_selected = state.slash_selected.saturating_sub(1);
                return true;
            }
            KeyCode::Down => {
                state.slash_selected = (state.slash_selected + 1).min(commands.len() - 1);
                return true;
            }
            KeyCode::PageUp => {
                state.slash_selected = state.slash_selected.saturating_sub(5);
                return true;
            }
            KeyCode::PageDown => {
                state.slash_selected = (state.slash_selected + 5).min(commands.len() - 1);
                return true;
            }
            KeyCode::Esc => {
                state.slash_dismissed = Some(state.composer.lines()[0].clone());
                return true;
            }
            KeyCode::Tab => {
                let name = commands[state.slash_selected].name;
                state.composer = composer::new();
                state.composer.insert_str(name);
                state.slash_selected = 0;
                return true;
            }
            KeyCode::Enter
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                let name = commands[state.slash_selected].name;
                if name == "/resume" {
                    state.composer = composer::new();
                    state.composer.insert_str("/resume");
                    if let (Some(submission), Some(session)) =
                        (state.begin(), state.session.clone())
                    {
                        effects.start(
                            client.clone(),
                            session,
                            submission,
                            state.generation,
                            send.clone(),
                        );
                    }
                    return true;
                }
                dispatch_command(name, state, effects, client, send);
                return true;
            }
            _ => {
                state.slash_selected = 0;
            }
        }
    }

    false
}
