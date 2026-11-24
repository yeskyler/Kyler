use crate::state::State;

use ratatui::{Frame, layout::Rect};

pub struct Command {
    pub name: &'static str,
    pub description: &'static str,
}
pub const COMMANDS: &[Command] = &[
    Command {
        name: "/model",
        description: "Model",
    },
    Command {
        name: "/permissions",
        description: "Permissions",
    },
    Command {
        name: "/mcp",
        description: "MCP",
    },
    Command {
        name: "/skills",
        description: "Skills",
    },
    Command {
        name: "/help",
        description: "Help",
    },
    Command {
        name: "/compact",
        description: "Compact conversation history",
    },
    Command {
        name: "/memory",
        description: "Review conversation for memory",
    },
    Command {
        name: "/resume",
        description: "Continue task",
    },
];
pub struct CommandMatch {
    pub name: String,
    pub description: String,
    pub group: &'static str,
    pub skill: Option<String>,
}

pub fn skill_alias(name: &str) -> String {
    if name == "review-agent" {
        return "/review".into();
    }
    if name == "review"
        || COMMANDS
            .iter()
            .any(|command| command.name.strip_prefix('/') == Some(name))
    {
        format!("/skill:{name}")
    } else {
        format!("/{name}")
    }
}

pub fn skill_for_alias(state: &State, alias: &str) -> Option<String> {
    let alias = alias.to_ascii_lowercase();
    if alias == "/review" {
        return Some("review-agent".into());
    }
    state
        .skills
        .iter()
        .find(|skill| skill_alias(&skill.name) == alias)
        .map(|skill| skill.name.clone())
}

pub fn query(state: &State) -> Option<(String, String)> {
    if state.composer.lines().len() != 1 {
        return None;
    }
    let text = &state.composer.lines()[0];
    let start = text
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    let (prefix, token) = text.split_at(start);
    if !token.starts_with('/')
        || !prefix
            .split_whitespace()
            .all(|part| part.starts_with('$') && kyler_core::skills::references(part).len() == 1)
    {
        return None;
    }
    Some((prefix.into(), token.into()))
}

pub fn matches(state: &State) -> Vec<CommandMatch> {
    if (state.approval_focus && state.pending_approval().is_some())
        || state.approval_details.is_some()
        || state.help
        || state.confirm_exit
        || state.mcp_open
        || state.permissions_open
        || state.model_open
        || state.skills_open
        || state.composer.lines().len() != 1
    {
        return Vec::new();
    }
    let Some((prefix, query)) = query(state) else {
        return Vec::new();
    };
    if state.slash_dismissed.as_ref() == Some(&state.composer.lines()[0]) {
        return Vec::new();
    }
    let mut commands = state
        .skills
        .iter()
        .filter(|skill| skill.status == kyler_protocol::api::skills::SkillStatus::Enabled)
        .map(|skill| CommandMatch {
            name: skill_alias(&skill.name),
            description: skill.label().to_owned(),
            group: if skill.source == kyler_protocol::api::skills::SkillSource::Builtin {
                "System skills"
            } else {
                "User skills"
            },
            skill: Some(skill.name.clone()),
        })
        .collect::<Vec<_>>();
    commands.extend(COMMANDS.iter().map(|command| CommandMatch {
        name: command.name.into(),
        description: command.description.into(),
        group: "Conversation",
        skill: None,
    }));
    let search = query.trim_start_matches('/').to_lowercase();
    let exact = commands
        .iter()
        .any(|command| command.name.eq_ignore_ascii_case(&query));
    let known_skill = skill_for_alias(state, &query);
    commands.retain(|command| {
        let matched = if exact {
            command.name.eq_ignore_ascii_case(&query)
        } else if let Some(name) = &known_skill {
            command.skill.as_ref() == Some(name)
        } else {
            command
                .name
                .trim_start_matches('/')
                .to_lowercase()
                .starts_with(&search)
                || (command.skill.is_some()
                    && (command.description.to_lowercase().starts_with(&search)
                        || (search.len() >= 3
                            && command.description.to_lowercase().contains(&search))))
        };
        matched && (prefix.is_empty() || command.skill.is_some())
    });
    commands.sort_by(|a, b| {
        let rank = |group| match group {
            "System skills" => 0,
            "User skills" => 1,
            _ => 2,
        };
        rank(a.group).cmp(&rank(b.group)).then_with(|| {
            if a.skill.is_some() && b.skill.is_some() {
                a.name.cmp(&b.name)
            } else {
                std::cmp::Ordering::Equal
            }
        })
    });
    commands
}

pub fn render(frame: &mut Frame, state: &State, input: Rect) -> Rect {
    let commands = matches(state);
    if commands.is_empty() {
        return Rect::default();
    }
    let selected = state.slash_selected.min(commands.len() - 1);
    let mut lines = Vec::new();
    let mut group = "";
    let mut selected_row = 0;
    let text = state.composer.lines().join("\n");
    for (index, command) in commands.iter().enumerate() {
        if group != command.group {
            group = command.group;
            lines.push(ratatui::text::Line::styled(
                group,
                ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
            ));
        }
        if index == selected {
            selected_row = lines.len();
        }
        let checked = command.skill.as_ref().is_some_and(|name| {
            text.split_whitespace()
                .any(|token| token == format!("${name}") || token == format!("@{name}"))
        });
        lines.push(super::panel::selected(
            format!(
                "{} {:<15} {}{}",
                if index == selected { ">" } else { " " },
                command.name,
                command.description,
                if checked { " ✓" } else { "" }
            ),
            index == selected,
        ));
    }
    super::panel::draw(
        frame,
        input,
        "Commands",
        lines,
        "↑↓ Select · Enter Open · Tab Complete · Esc Close",
        false,
        super::panel::Position::Selected(selected_row),
    )
    .0
}
