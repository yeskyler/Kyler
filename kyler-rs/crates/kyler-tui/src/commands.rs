//! Dispatch local slash commands without sending them to the model.
use crate::{
    components::composer,
    effects::{Effects, Sender},
    state::{Panel, State},
};

use kyler_client::Client;

pub(crate) fn dispatch_command(
    name: &str,
    state: &mut State,
    effects: &mut Effects,
    client: &Client,
    send: &Sender,
) {
    let parent = (state.composer.lines().join("\n"), state.slash_selected);
    state.composer = composer::new();
    state.slash_selected = 0;
    match name {
        "/model" => crate::components::model_picker::open(state, effects, client, send),
        "/permissions" => crate::components::permission_picker::open(state),
        "/skills" => crate::components::skills_panel::open(state, effects, client, send),
        "/mcp" => state.open_panel(Panel::Mcp),
        _ => state.open_panel(Panel::Help),
    }
    state.panel_parent = Some(parent);
}
