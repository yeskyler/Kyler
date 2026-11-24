//! Dispatch local slash commands without sending them to the model.
use crate::agent::AgentHandle;
use crate::{
    components::composer,
    effects::{Effects, Sender},
    state::{Panel, State},
};

pub(crate) fn starts_turn(name: &str) -> bool {
    matches!(name, "/resume" | "/compact" | "/memory")
}

pub(crate) fn dispatch_command(
    name: &str,
    state: &mut State,
    effects: &mut Effects,
    client: &AgentHandle,
    send: &Sender,
) {
    if let Some(skill_name) = crate::components::command_picker::skill_for_alias(state, name) {
        if !state.skills.iter().any(|skill| {
            skill.name == skill_name
                && skill.status == kyler_protocol::api::skills::SkillStatus::Enabled
        }) {
            state.notice("Enable this skill in Skills before selecting it.");
            return;
        }
        let prefix = crate::components::command_picker::query(state)
            .map(|draft| draft.0)
            .unwrap_or_default();
        crate::components::skill_picker::insert(state, &skill_name, &prefix);
        return;
    }
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
