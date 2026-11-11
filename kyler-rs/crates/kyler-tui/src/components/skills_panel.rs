use super::panel::{self, Position};
use crate::agent::AgentHandle;
use crate::{
    components::composer::clean,
    effects::{Effects, Sender},
    state::{Panel, State},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
};

use kyler_protocol::api::skills::{SkillAction, SkillSource, SkillStatus};

pub fn open(state: &mut State, effects: &mut Effects, client: &AgentHandle, send: &Sender) {
    state.open_panel(Panel::Skills);
    state.skill_details = None;
    state.skills_error = None;
    state.skills_scroll = 0;
    if let Some(session) = state.session.clone() {
        state.skills_busy = true;
        effects.skills(
            client.clone(),
            session,
            state.generation,
            send.clone(),
            false,
        );
    } else {
        state.skills_error = Some("Wait for the session to finish loading.".into());
    }
}
pub fn handle_key(
    key: KeyEvent,
    state: &mut State,
    effects: &mut Effects,
    client: &AgentHandle,
    send: &Sender,
) {
    if key.code == KeyCode::Esc {
        if state.skill_details.is_some() {
            state.skill_details = None;
            state.skills_scroll = 0;
        } else {
            state.skills_open = false;
        }
        return;
    }
    if state.skills_busy {
        return;
    }
    match key.code {
        KeyCode::Up if state.skill_details.is_some() => {
            state.skills_scroll = state.skills_scroll.saturating_sub(1)
        }
        KeyCode::Down if state.skill_details.is_some() => state.skills_scroll += 1,
        KeyCode::PageUp if state.skill_details.is_some() => {
            state.skills_scroll = state.skills_scroll.saturating_sub(8)
        }
        KeyCode::PageDown if state.skill_details.is_some() => state.skills_scroll += 8,
        KeyCode::PageUp => state.skills_selected = state.skills_selected.saturating_sub(5),
        KeyCode::PageDown => {
            state.skills_selected = (state.skills_selected + 5).min(state.skills.len() + 1)
        }
        KeyCode::Up => state.skills_selected = state.skills_selected.saturating_sub(1),
        KeyCode::Down => {
            state.skills_selected = (state.skills_selected + 1).min(state.skills.len() + 1)
        }
        KeyCode::Char('r') if key.modifiers == KeyModifiers::NONE => {
            if let Some(session) = state.session.clone() {
                state.skills_busy = true;
                state.skill_details = None;
                effects.skills(
                    client.clone(),
                    session,
                    state.generation,
                    send.clone(),
                    true,
                );
            }
        }
        KeyCode::Enter if state.skill_details.is_none() && state.skills_selected < 2 => {
            let name = if state.skills_selected == 0 {
                "skill-creator"
            } else {
                "skill-installer"
            };
            if !state
                .skills
                .iter()
                .any(|skill| skill.name == name && skill.status == SkillStatus::Enabled)
            {
                state.skills_error = Some(format!("Enable {name} before using this action."));
                return;
            }
            if state.composer.lines().iter().any(|line| !line.is_empty()) {
                state.skills_error =
                    Some("Clear the current draft before starting a skill task.".into());
                return;
            }
            state.skills_open = false;
            state.panel_parent = None;
            state.composer.insert_str(format!("${name} "));
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            let Some(skill) = state
                .skills_selected
                .checked_sub(2)
                .and_then(|index| state.skills.get(index))
                .cloned()
            else {
                return;
            };
            let Some(session) = state.session.clone() else {
                return;
            };
            if matches!(skill.status, SkillStatus::Invalid | SkillStatus::Shadowed) {
                state.skills_error =
                    Some(skill.error.unwrap_or_else(|| {
                        "This skill is shadowed by a higher-priority copy.".into()
                    }));
                return;
            }
            if key.code == KeyCode::Enter
                || (skill.status != SkillStatus::Enabled
                    && state
                        .skill_details
                        .as_ref()
                        .is_none_or(|d| d.info.fingerprint != skill.fingerprint))
            {
                state.skills_busy = true;
                effects.skill_details(
                    client.clone(),
                    session,
                    skill.id,
                    state.generation,
                    send.clone(),
                );
            } else {
                let action = if skill.status == SkillStatus::Enabled {
                    SkillAction::Disable
                } else {
                    SkillAction::Enable
                };
                state.skills_busy = true;
                effects.skill_action(
                    client.clone(),
                    session,
                    skill.id,
                    action,
                    skill.fingerprint,
                    state.generation,
                    send.clone(),
                );
            }
        }
        _ => {}
    }
}
pub fn render(frame: &mut Frame, state: &mut State, input: Rect) -> Rect {
    let mut lines = Vec::new();
    let mut selected_row = 0;
    if let Some(details) = &state.skill_details {
        lines.push(Line::styled(
            clean(&details.info.name),
            Style::default().fg(Color::Cyan),
        ));
        lines.push(Line::from(clean(&details.info.description)));
        lines.push(Line::from(match details.info.source {
            SkillSource::Builtin => "Source: Built-in",
            SkillSource::User => "Source: User",
            SkillSource::Project => "Source: Project",
        }));
        lines.push(Line::from(clean(&details.info.path)));
        lines.push(Line::from(format!(
            "Fingerprint: {}",
            details.info.fingerprint
        )));
        lines.push(Line::from(
            "Review before enabling. Sandbox permissions stay unchanged.",
        ));
        lines.extend(
            clean(&details.instructions)
                .lines()
                .map(|l| Line::from(l.to_owned())),
        );
        if !details.info.resources.is_empty() {
            lines.push(Line::from("Resources"));
            for resource in &details.info.resources {
                lines.push(Line::from(clean(resource)));
            }
        }
    } else {
        for (index, label) in ["Create skill…", "Install skill…"].iter().enumerate() {
            if state.skills_selected == index {
                selected_row = lines.len();
            }
            lines.push(panel::selected(
                format!(
                    "{} {label}",
                    if state.skills_selected == index {
                        ">"
                    } else {
                        " "
                    }
                ),
                state.skills_selected == index,
            ));
        }
        lines.push(Line::from("──────────────────────────"));
        for (index, skill) in state.skills.iter().enumerate() {
            let index = index + 2;
            if index == state.skills_selected {
                selected_row = lines.len();
            }
            let status = match skill.status {
                SkillStatus::Enabled => "Enabled",
                SkillStatus::Disabled => "Disabled",
                SkillStatus::NeedsApproval => "Needs approval",
                SkillStatus::Shadowed => "Shadowed",
                SkillStatus::Invalid => "Invalid",
            };
            lines.push(panel::selected(
                format!(
                    "{} {} · {} · {status}",
                    if index == state.skills_selected {
                        ">"
                    } else {
                        " "
                    },
                    clean(&skill.name),
                    match skill.source {
                        SkillSource::Builtin => "Built-in",
                        SkillSource::User => "User",
                        SkillSource::Project => "Project",
                    }
                ),
                index == state.skills_selected,
            ));
        }
        if state.skills.is_empty() && !state.skills_busy {
            lines.push(Line::from("No skills found"));
        }
    }
    if state.skills_busy {
        lines.push(Line::from("Loading…"));
    }
    if let Some(error) = &state.skills_error {
        lines.push(Line::styled(clean(error), Style::default().fg(Color::Red)));
    }
    let details = state.skill_details.is_some();
    let (area, scroll) = panel::draw(
        frame,
        input,
        if details {
            "Skills › Review"
        } else {
            "Skills"
        },
        lines,
        if details {
            "↑↓ Scroll · Space Enable/Disable · Esc Back"
        } else {
            "Enter Review · Space Toggle · r Reload"
        },
        details,
        if details {
            Position::Scroll(state.skills_scroll)
        } else {
            Position::Selected(selected_row)
        },
    );
    if details {
        state.skills_scroll = scroll;
    }
    area
}
