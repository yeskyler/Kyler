//! Mouse hit testing and scrolling for panels, composer and transcript.
use super::scroll_panel;
use crate::{layout::View, state::State};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

pub(super) fn handle(mouse: MouseEvent, state: &mut State, view: &View) -> bool {
    if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
        if let Some(index) = view.reasoning_at(mouse.column, mouse.row, state) {
            state.toggle_reasoning(index);
        }
    }
    let delta = match mouse.kind {
        MouseEventKind::ScrollUp => -3,
        MouseEventKind::ScrollDown => 3,
        _ => 0,
    };
    if delta != 0 {
        if view.panel.contains((mouse.column, mouse.row).into()) {
            scroll_panel(state, delta);
            return false;
        }
        if view.input.contains((mouse.column, mouse.row).into()) {
            state.composer.scroll((delta as i16, 0));
        } else if view.transcript.contains((mouse.column, mouse.row).into()) {
            state.scroll_by(delta);
        }
    }

    false
}
