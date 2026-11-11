use super::{
    composer::clean,
    panel::{self, Position},
};
use crate::agent::AgentHandle;
use crate::{
    effects::{Effects, Sender},
    state::{Panel, State},
};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
};

use kyler_protocol::tokens::format_tokens;

pub fn refresh(
    state: &mut State,
    effects: &mut Effects,
    client: &AgentHandle,
    send: &Sender,
    reload: bool,
    selection: Option<Option<String>>,
) {
    if state.model_busy {
        return;
    }
    if let Some(session) = state.session.clone() {
        state.model_busy = true;
        state.model_error = None;
        effects.models(
            client.clone(),
            session,
            state.generation,
            send.clone(),
            reload,
            selection,
        );
    }
}
pub fn open(state: &mut State, effects: &mut Effects, client: &AgentHandle, send: &Sender) {
    state.open_panel(Panel::Model);
    if let Some(index) = state
        .session_model
        .as_ref()
        .and_then(|s| s.selected.as_ref())
        .and_then(|selected| {
            state
                .model_catalog
                .as_ref()
                .and_then(|c| c.models.iter().position(|m| m.profile == selected.profile))
        })
    {
        state.model_selected = index;
    }
    refresh(state, effects, client, send, false, None);
}
pub fn handle_key(
    key: KeyEvent,
    state: &mut State,
    effects: &mut Effects,
    client: &AgentHandle,
    send: &Sender,
) {
    if let Some(form) = state.model_form.as_mut() {
        if key.code == KeyCode::Esc {
            state.model_form = None;
        } else {
            form.key(key);
        }
        return;
    }
    match key.code {
        KeyCode::Char('e') => {
            let model = state
                .model_catalog
                .as_ref()
                .and_then(|c| c.models.get(state.model_selected))
                .map(|m| m.model.clone())
                .unwrap_or_default();
            state.model_form = Some(super::model_config::ModelConfigForm::new(model));
        }
        KeyCode::Esc if state.model_details => {
            state.model_details = false;
            state.model_scroll = 0;
        }
        KeyCode::Esc => state.model_open = false,
        KeyCode::Char('i') => {
            state.model_details = !state.model_details;
            state.model_scroll = 0;
        }
        KeyCode::Up if state.model_details => {
            state.model_scroll = state.model_scroll.saturating_sub(1)
        }
        KeyCode::Down if state.model_details => state.model_scroll += 1,
        KeyCode::PageUp if !state.model_details => {
            state.model_selected = state.model_selected.saturating_sub(5)
        }
        KeyCode::PageDown if !state.model_details => {
            state.model_selected = (state.model_selected + 5).min(
                state
                    .model_catalog
                    .as_ref()
                    .map(|c| c.models.len().saturating_sub(1))
                    .unwrap_or(0),
            )
        }
        KeyCode::PageUp => state.model_scroll = state.model_scroll.saturating_sub(5),
        KeyCode::PageDown => state.model_scroll += 5,
        KeyCode::Up => state.model_selected = state.model_selected.saturating_sub(1),
        KeyCode::Down => {
            state.model_selected = (state.model_selected + 1).min(
                state
                    .model_catalog
                    .as_ref()
                    .map(|c| c.models.len().saturating_sub(1))
                    .unwrap_or(0),
            )
        }
        KeyCode::Char('r') => refresh(state, effects, client, send, true, None),
        KeyCode::Char('d') if !state.model_busy => {
            state.model_applying = true;
            refresh(state, effects, client, send, false, Some(None));
        }
        KeyCode::Enter if !state.model_busy => {
            if let Some(profile) = state
                .model_catalog
                .as_ref()
                .and_then(|c| c.models.get(state.model_selected))
                .map(|m| m.profile.clone())
            {
                state.model_applying = true;
                refresh(state, effects, client, send, false, Some(Some(profile)));
            }
        }
        _ => {}
    }
}
pub fn render(frame: &mut Frame, state: &mut State, input: Rect) -> Rect {
    if let Some(form) = &state.model_form {
        return form.render(frame, input);
    }
    let mut lines = Vec::new();
    let mut selected_row = 0;
    if state.model_details {
        if let Some(model) = state
            .model_catalog
            .as_ref()
            .and_then(|c| c.models.get(state.model_selected))
        {
            let capability = |value: Option<bool>| match value {
                Some(true) => "yes",
                Some(false) => "no",
                None => "undeclared",
            };
            lines.extend([
                Line::from(clean(&model.model)),
                Line::from(format!(
                    "Images: {} · Tools: {}",
                    capability(model.capabilities.images),
                    capability(model.capabilities.tools)
                )),
                Line::from(format!(
                    "Structured output: {} · Tokenizer: {:?}",
                    capability(model.capabilities.structured_output),
                    model.capabilities.tokenizer
                )),
                Line::from(format!("Profile: {}", clean(&model.profile))),
                Line::from(format!("Provider: {}", clean(&model.provider))),
                Line::from(format!(
                    "Context {} / output {} tokens",
                    format_tokens(model.context_window_tokens as u64),
                    format_tokens(u64::from(model.output_tokens))
                )),
            ]);
        }
        if let Some(session) = &state.session_model {
            if let Some(active) = &session.active {
                lines.push(Line::from(format!("Running: {}", clean(&active.model))));
            }
            if let Some(selected) = &session.selected {
                lines.push(Line::from(format!("Next task: {}", clean(&selected.model))));
            }
        }
    } else if let Some(catalog) = &state.model_catalog {
        for (index, model) in catalog.models.iter().enumerate() {
            if index == state.model_selected {
                selected_row = lines.len();
            }
            let checked = state
                .session_model
                .as_ref()
                .and_then(|s| s.selected.as_ref())
                .is_some_and(|s| s.profile == model.profile);
            lines.push(panel::selected(
                format!(
                    "{} {}{}",
                    if index == state.model_selected {
                        ">"
                    } else {
                        " "
                    },
                    clean(&model.model),
                    if checked { " ✓" } else { "" }
                ),
                index == state.model_selected,
            ));
        }
        if catalog.models.is_empty() {
            lines.push(Line::from("No models"));
        }
    }
    if state.model_busy {
        lines.push(Line::from("Loading…"));
    }
    if let Some(error) = &state.model_error {
        lines.push(Line::styled(clean(error), Style::default().fg(Color::Red)));
    }
    if let Some(error) = state.session_model.as_ref().and_then(|s| s.error.as_ref()) {
        lines.push(Line::styled(clean(error), Style::default().fg(Color::Red)));
    }
    let position = if state.model_details {
        Position::Scroll(state.model_scroll)
    } else {
        Position::Selected(selected_row)
    };
    let (area, scroll) = panel::draw(
        frame,
        input,
        if state.model_details {
            "Model › Details"
        } else {
            "Model"
        },
        lines,
        if state.model_details {
            "↑↓ Scroll · Esc Back"
        } else {
            "Enter Apply · e Configure · i Details · r Reload"
        },
        state.model_details,
        position,
    );
    if state.model_details {
        state.model_scroll = scroll;
    }
    area
}
