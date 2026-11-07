use super::{ready, render, result};
use crate::{
    app,
    components::{command_picker::COMMANDS, status, transcript},
    effects::{Effects, Event},
    layout::View,
    state::{Entry, Phase},
};

use crossterm::event::{Event as Input, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

use kyler_client::Client;
use kyler_config::ClientSettings;

#[tokio::test]
async fn mcp_command_opens_management_panel_without_submitting_to_model() {
    use kyler_protocol::api::mcp::{McpServerInfo, McpStatus, McpToolInfo};

    let client = Client::new(ClientSettings::default()).unwrap();
    let mut state = ready();
    state.mcp = vec![McpServerInfo {
        removable: false,
        id: "expense".into(),
        transport: "stdio".into(),
        status: McpStatus::Connected,
        error: None,
        tools: vec![McpToolInfo {
            name: "list_expenses".into(),
            description: "List expense records".into(),
        }],
    }];
    let mut effects = Effects::default();
    let (send, _receive) = tokio::sync::mpsc::channel(16);
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let welcome = render(&mut terminal, &mut view, &mut state);
    assert!(!welcome.contains("MCP"));
    assert!(!welcome.contains("How can I help"));
    assert!(welcome.contains("Model") && welcome.contains("Directory"));
    state.composer.insert_str("/mcp");
    app::handle_input(
        Input::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(state.mcp_open);
    assert!(state.pending.is_none());
    assert!(state.entries.is_empty());
    assert_eq!(state.phase, Phase::Ready);
    let panel = render(&mut terminal, &mut view, &mut state);
    assert!(panel.contains("MCP") && panel.contains("expense") && panel.contains("Connected"));
    app::handle_input(
        Input::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    let details = render(&mut terminal, &mut view, &mut state);
    assert!(details.contains("list_expenses") && details.contains("List expense records"));
    app::handle_input(
        Input::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(!state.mcp_details && state.mcp_open);
    app::handle_input(
        Input::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(!state.mcp_open);
}

#[test]
fn slash_menu_filters_completes_and_dispatches_commands_without_a_model_request() {
    let client = Client::new(ClientSettings::default()).unwrap();
    let mut state = ready();
    let mut effects = Effects::default();
    let (send, _receive) = tokio::sync::mpsc::channel(16);
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    assert!(transcript::lines(&[]).is_empty());
    state.composer.insert_str("/");
    let menu = render(&mut terminal, &mut view, &mut state);
    assert!(menu.contains("Commands") && menu.contains("/mcp") && menu.contains("/help"));
    let bottom = view.input.bottom();
    let key = |code| Input::Key(KeyEvent::new(code, KeyModifiers::NONE));
    let help_index = COMMANDS
        .iter()
        .position(|command| command.name == "/help")
        .unwrap();
    for _ in 0..help_index {
        app::handle_input(
            key(KeyCode::Down),
            &mut state,
            &mut effects,
            &client,
            &send,
            &view,
        );
    }
    app::handle_input(
        key(KeyCode::Enter),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(state.help && state.pending.is_none() && state.entries.is_empty());
    state.help = false;
    state.composer.insert_str("/m");
    let filtered = render(&mut terminal, &mut view, &mut state);
    assert!(filtered.contains("/mcp") && !filtered.contains("/help"));
    app::handle_input(
        key(KeyCode::Tab),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert_eq!(state.composer.lines(), &["/model"]);
    app::handle_input(
        key(KeyCode::Esc),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(!render(&mut terminal, &mut view, &mut state).contains("Commands"));
    assert_eq!(view.input.bottom(), bottom);
    assert_eq!(state.phase, Phase::Ready);
    assert!(state.pending.is_none() && state.entries.is_empty());
}

#[tokio::test]
async fn skills_command_requires_review_before_project_approval_and_does_not_submit() {
    use kyler_protocol::api::skills::{SkillDetails, SkillInfo, SkillSource, SkillStatus};

    let client = Client::new(ClientSettings::default()).unwrap();
    let mut state = ready();
    let mut effects = Effects::default();
    let (send, _receive) = tokio::sync::mpsc::channel(16);
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    state.composer.insert_str("/sk");
    assert!(render(&mut terminal, &mut view, &mut state).contains("/skills"));
    let key = |code| Input::Key(KeyEvent::new(code, KeyModifiers::NONE));
    app::handle_input(
        key(KeyCode::Enter),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(state.skills_open && state.pending.is_none() && state.entries.is_empty());
    let info = SkillInfo {
        id: "a".repeat(64),
        name: "review".into(),
        description: "Review changes".into(),
        source: SkillSource::Project,
        path: "/project/.kyler/skills/review".into(),
        fingerprint: "b".repeat(64),
        status: SkillStatus::NeedsApproval,
        resources: vec!["references/guide.md".into()],
        error: None,
    };
    app::apply(
        state.generation,
        Event::Skills(vec![info.clone()]),
        &mut state,
        &mut effects,
        &client,
        &send,
    );
    let panel = render(&mut terminal, &mut view, &mut state);
    assert!(panel.contains("Needs approval"));
    state.skills_selected = 2;
    app::handle_input(
        key(KeyCode::Char(' ')),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(state.skills_busy && state.skill_details.is_none());
    assert_eq!(state.skills[0].status, SkillStatus::NeedsApproval);
    app::apply(
        state.generation,
        Event::SkillDetails(SkillDetails {
            info,
            instructions: "Review instructions".into(),
        }),
        &mut state,
        &mut effects,
        &client,
        &send,
    );
    assert!(render(&mut terminal, &mut view, &mut state).contains("Review instructions"));
    app::handle_input(
        key(KeyCode::Char(' ')),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(state.skills_busy && state.pending.is_none());
    assert_eq!(
        state.skills[0].status,
        SkillStatus::NeedsApproval,
        "wait for server confirmation"
    );
    app::handle_input(
        key(KeyCode::Esc),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    app::handle_input(
        key(KeyCode::Esc),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(!state.skills_open);
}

#[tokio::test]
async fn model_menu_is_local_and_running_model_is_distinct_from_next_selection() {
    use kyler_protocol::api::models::{ModelCatalog, ModelInfo, SessionModel};

    let client = Client::new(ClientSettings::default()).unwrap();
    let mut state = ready();
    let mut effects = Effects::default();
    let (send, _receive) = tokio::sync::mpsc::channel(16);
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
    let key = |code| Input::Key(KeyEvent::new(code, KeyModifiers::NONE));
    state.composer.insert_str("/mod");
    assert!(render(&mut terminal, &mut view, &mut state).contains("/model"));
    app::handle_input(
        key(KeyCode::Enter),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(state.model_open && state.model_busy && state.pending.is_none());
    assert!(state.entries.is_empty());
    let current = ModelInfo {
        capabilities: Default::default(),
        profile: "first".into(),
        provider: "fixture".into(),
        model: "Running model".into(),
        revision: 1,
        context_window_tokens: 64000,
        sliding_window_tokens: 38400,
        output_tokens: 2048,
    };
    let next = ModelInfo {
        capabilities: Default::default(),
        profile: "second".into(),
        model: "Next model".into(),
        revision: 2,
        ..current.clone()
    };
    app::apply(
        state.generation,
        Event::Models(
            ModelCatalog {
                revision: 2,
                default_profile: "first".into(),
                models: vec![current.clone(), next.clone()],
            },
            SessionModel {
                profile: Some(next.profile.clone()),
                selected: Some(next.clone()),
                active: Some(current),
                error: None,
            },
        ),
        &mut state,
        &mut effects,
        &client,
        &send,
    );
    state.phase = Phase::Running;
    state.run = Some("run".into());
    assert_eq!(state.displayed_model(), "Running model");
    assert!(!status::text(&state).contains("Next:"));
    let panel = render(&mut terminal, &mut view, &mut state);
    assert!(panel.contains("Model") && !panel.contains("Context 64000"));
    app::handle_input(
        key(KeyCode::Char('i')),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    let details = render(&mut terminal, &mut view, &mut state);
    assert!(details.contains("Context 64k / output 2.048k tokens"));
    app::handle_input(
        key(KeyCode::Esc),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    app::handle_input(
        key(KeyCode::Esc),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(!state.model_open && state.pending.is_none());
    state.stream(result("done"));
    assert_eq!(state.displayed_model(), "Next model");
    assert!(!status::text(&state).contains("Next:"));
    terminal
        .resize(ratatui::layout::Rect::new(0, 0, 25, 8))
        .unwrap();
    state.model_open = true;
    render(&mut terminal, &mut view, &mut state);
}

#[tokio::test]
async fn permissions_menu_confirms_full_access_and_approval_defaults_to_deny() {
    use kyler_protocol::api::permissions::{
        ApprovalRecord, ApprovalStatus, PermissionMode, PermissionOption, PermissionSnapshot,
        PermissionState,
    };

    let client = Client::new(ClientSettings::default()).unwrap();
    let mut state = ready();
    let mut effects = Effects::default();
    let (send, _receive) = tokio::sync::mpsc::channel(32);
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 32)).unwrap();
    state.permissions = Some(PermissionState {
        sandbox: None,
        approval: Default::default(),
        selected: PermissionSnapshot {
            approval: None,
            mode: PermissionMode::ReadOnly,
            revision: "one".into(),
        },
        active: None,
        options: PermissionMode::ALL
            .into_iter()
            .map(|mode| PermissionOption {
                mode,
                available: true,
                description: "Within server ceiling".into(),
            })
            .collect(),
        workspace: Some("/project".into()),
        ceiling: "Workspace write; commands allowed; network blocked".into(),
    });
    let key = |code| Input::Key(KeyEvent::new(code, KeyModifiers::NONE));
    state.composer.insert_str("/permissions");
    app::handle_input(
        key(KeyCode::Enter),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    let menu = render(&mut terminal, &mut view, &mut state);
    assert!(
        menu.contains("Read-only")
            && menu.contains("Workspace access")
            && menu.contains("Full access")
            && !menu.contains("Auto review")
            && !menu.contains("Reviewer")
            && !menu.contains("on-request")
    );
    assert!(state.permissions_open && state.pending.is_none());
    for _ in 0..2 {
        app::handle_input(
            key(KeyCode::Down),
            &mut state,
            &mut effects,
            &client,
            &send,
            &view,
        );
    }
    app::handle_input(
        key(KeyCode::Enter),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(state.permissions_confirm && !state.permissions_busy);
    app::handle_input(
        key(KeyCode::Esc),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(state.permissions_open && !state.permissions_confirm);
    app::handle_input(
        key(KeyCode::Esc),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(!state.permissions_open);
    state.composer = crate::components::composer::new();
    let record = ApprovalRecord {
        id: "00000000-0000-4000-8000-000000000002".into(),
        run_id: "run".into(),
        session_id: state.session.clone().unwrap(),
        origin_session_id: "child".into(),
        execution_id: "execution".into(),
        tool_call_id: "tool".into(),
        tool: "apply_patch".into(),
        operation: "Modify files".into(),
        preview: "*** Add File: file.rs\n+fn main() {}".into(),
        requires_user: true,
        working_directory: Some("/project".into()),
        policy: PermissionSnapshot {
            approval: None,
            mode: PermissionMode::WorkspaceAccess,
            revision: "one".into(),
        },
        digest: "digest".into(),
        status: ApprovalStatus::Pending,
        reason: "User approval required".into(),
        created_at: 0,
        expires_at: i64::MAX,
    };
    state.phase = Phase::Running;
    state.run = Some("run".into());
    state.update_approval(record.clone());
    assert!(state.approval_focus);
    let display = render(&mut terminal, &mut view, &mut state);
    assert!(
        display.contains("Approval") && display.contains("Deny") && display.contains("Allow once")
    );
    assert!(!display.contains("Allow for session"));
    assert_eq!(view.panel.bottom(), view.input.y);
    let transcript = crate::components::transcript::lines(&state.entries)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!transcript.contains("User approval required"));
    assert!(!transcript.contains("*** Add File: file.rs"));
    assert_eq!(view.panel.width, view.input.width);
    terminal
        .resize(ratatui::layout::Rect::new(0, 0, 20, 12))
        .unwrap();
    let narrow = render(&mut terminal, &mut view, &mut state);
    assert!(narrow.contains("Deny") && narrow.contains("Allow once"));
    assert_eq!(view.panel.bottom(), view.input.y);
    terminal
        .resize(ratatui::layout::Rect::new(0, 0, 100, 32))
        .unwrap();
    for code in [
        KeyCode::Right,
        KeyCode::Left,
        KeyCode::Tab,
        KeyCode::BackTab,
    ] {
        app::handle_input(key(code), &mut state, &mut effects, &client, &send, &view);
        assert!(
            !state.approval_busy,
            "Selection alone must not send a decision"
        );
        let index = state.pending_approval().unwrap();
        assert!(
            matches!(&state.entries[index], Entry::Approval { choice, .. } if *choice == if matches!(code, KeyCode::Right | KeyCode::Tab) { 0 } else { 2 })
        );
    }
    app::apply(
        state.generation,
        Event::ApprovalError("UNIQUE_APPROVAL_FAILURE".into()),
        &mut state,
        &mut effects,
        &client,
        &send,
    );
    let failed = render(&mut terminal, &mut view, &mut state);
    assert!(failed.contains("UNIQUE_APPROVAL_FAILURE"));
    assert!(
        !crate::components::transcript::lines(&state.entries)
            .iter()
            .any(|line| line.to_string().contains("UNIQUE_APPROVAL_FAILURE"))
    );
    app::handle_input(
        key(KeyCode::Char('e')),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    let details = render(&mut terminal, &mut view, &mut state);
    assert!(details.contains("Approval › Details"));
    assert_eq!(view.panel.bottom(), view.input.y);
    assert_eq!(view.panel.width, view.input.width);
    app::handle_input(
        key(KeyCode::Enter),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(
        !state.approval_busy,
        "Viewing details must not approve the operation"
    );
    app::handle_input(
        key(KeyCode::Esc),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(state.approval_details.is_none() && state.approval_focus);

    app::handle_input(
        key(KeyCode::Enter),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(state.approval_busy);
    let mut denied = record.clone();
    denied.status = ApprovalStatus::Denied;
    app::apply(
        state.generation,
        Event::Approval(denied),
        &mut state,
        &mut effects,
        &client,
        &send,
    );
    assert!(state.pending_approval().is_none() && !state.approval_focus);
    state.update_approval(record.clone()); // Stale polling must not resurrect a resolved request.
    assert!(state.pending_approval().is_none());
    assert!(state.pending.is_none());
    for digit in ['y', 'Y', 'n', 'N'] {
        let mut pending = record.clone();
        pending.id = format!("shortcut-{digit}");
        pending.tool = "save_skill".into();
        pending.preview = serde_json::json!({"name":"support","scope":"user","files":{"SKILL.md":"UNIQUE_LONG_SKILL_BODY".repeat(400)}}).to_string();
        state.composer.insert_str("Draft ");
        state.update_approval(pending.clone());
        assert!(
            !state.approval_focus,
            "An approval must not steal focus while typing a draft"
        );
        let screen = render(&mut terminal, &mut view, &mut state);
        assert!(screen.contains("Create user skill: support"));
        assert!(!screen.contains("UNIQUE_LONG_SKILL_BODY"));
        state.approval_focus = false;
        app::handle_input(
            key(KeyCode::Char(digit)),
            &mut state,
            &mut effects,
            &client,
            &send,
            &view,
        );
        assert!(
            !state.approval_busy,
            "Typing into the composer must not approve"
        );
        state.composer = crate::components::composer::new();
        state.approval_focus = true;
        app::handle_input(
            key(KeyCode::Char(digit)),
            &mut state,
            &mut effects,
            &client,
            &send,
            &view,
        );
        assert!(state.approval_busy);
        let index = state.pending_approval().unwrap();
        assert!(
            matches!(&state.entries[index], Entry::Approval { choice, .. } if *choice == if digit.eq_ignore_ascii_case(&'y') { 0 } else { 2 })
        );
        pending.status = ApprovalStatus::Denied;
        app::apply(
            state.generation,
            Event::Approval(pending),
            &mut state,
            &mut effects,
            &client,
            &send,
        );
    }
    terminal
        .resize(ratatui::layout::Rect::new(0, 0, 25, 14))
        .unwrap();
    state.permissions_open = true;
    render(&mut terminal, &mut view, &mut state);
}

#[test]
fn every_surface_anchors_to_composer_and_restores_the_scrolled_transcript() {
    use crate::state::Panel;

    let mut state = ready();
    state.entries = (0..60)
        .map(|n| Entry::Message {
            role: "assistant".into(),
            content: format!("History message {n}"),
        })
        .collect();
    state.transcript_version += 1;
    state.follow = false;
    state.scroll = 13;
    state.composer.insert_str("draft");
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let before = render(&mut terminal, &mut view, &mut state);
    let input = view.input;
    let scroll = state.scroll;
    for kind in [
        Panel::Model,
        Panel::Permissions,
        Panel::Mcp,
        Panel::Skills,
        Panel::Help,
    ] {
        state.open_panel(kind);
        render(&mut terminal, &mut view, &mut state);
        assert_eq!(view.panel.x, view.input.x);
        assert_eq!(view.panel.bottom(), view.input.y);
        assert_eq!(view.panel.width, 52);
        assert!(view.panel.height <= view.input.y / 2);
        assert_eq!(view.input, input);
        assert_eq!(state.scroll, scroll);
        assert_eq!(state.composer.lines(), &["draft"]);
    }
    state.open_panel(Panel::Help);
    state.help = false;
    let after = render(&mut terminal, &mut view, &mut state);
    assert_eq!(before, after);
    state.composer = crate::components::composer::new();
    state.composer.insert_str("/");
    render(&mut terminal, &mut view, &mut state);
    assert_eq!(view.panel.bottom(), view.input.y);
    assert_eq!(view.panel.width, 52);
    for (width, height) in [(80, 24), (30, 16), (20, 12)] {
        terminal.backend_mut().resize(width, height);
        terminal
            .resize(ratatui::layout::Rect::new(0, 0, width, height))
            .unwrap();
        render(&mut terminal, &mut view, &mut state);
        assert_eq!(view.panel.bottom(), view.input.y);
        assert!(view.panel.right() <= width && view.panel.y < view.input.y);
    }
}

#[tokio::test]
async fn nested_panels_return_to_command_query_and_footer_labels_do_not_open_menus() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    let client = Client::new(ClientSettings {
        server_url: "http://127.0.0.1:1".into(),
        ..Default::default()
    })
    .unwrap();
    let mut state = ready();
    let mut effects = Effects::default();
    let (send, _receive) = tokio::sync::mpsc::channel(32);
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let key = |code| Input::Key(KeyEvent::new(code, KeyModifiers::NONE));
    state.composer.insert_str("/m");
    state.slash_selected = 1; // /mcp follows /model.
    render(&mut terminal, &mut view, &mut state);
    app::handle_input(
        key(KeyCode::Enter),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(state.mcp_open && !state.model_open);
    app::handle_input(
        key(KeyCode::Enter),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(state.mcp_details);
    render(&mut terminal, &mut view, &mut state);
    assert_eq!(view.panel.width, view.input.width);
    assert_eq!(view.panel.bottom(), view.input.y);
    app::handle_input(
        key(KeyCode::Esc),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(state.mcp_open && !state.mcp_details);
    app::handle_input(
        key(KeyCode::Esc),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(!state.mcp_open);
    assert_eq!(state.composer.lines(), &["/m"]);
    assert_eq!(state.slash_selected, 1);
    app::handle_input(
        key(KeyCode::Esc),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(crate::components::command_picker::matches(&state).is_empty());
    render(&mut terminal, &mut view, &mut state);
    let click = |x, y| {
        Input::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    };
    for column in view.input.x..view.input.right() {
        app::handle_input(
            click(column, view.input.bottom()),
            &mut state,
            &mut effects,
            &client,
            &send,
            &view,
        );
        assert!(!state.permissions_open && !state.model_open);
    }
    assert!(!status::text(&state).contains('▾'));
    assert_eq!(state.composer.lines(), &["/m"]);
    assert!(state.pending.is_none() && state.entries.is_empty());
}

#[test]
fn long_lists_keep_selection_visible_and_details_scroll_without_moving_history() {
    use crate::state::Panel;

    use crossterm::event::{MouseEvent, MouseEventKind};

    use kyler_protocol::api::models::{ModelCatalog, ModelInfo};

    let client = Client::new(ClientSettings::default()).unwrap();
    let mut state = ready();
    let mut effects = Effects::default();
    let (send, _receive) = tokio::sync::mpsc::channel(16);
    state.model_catalog = Some(ModelCatalog {
        revision: 1,
        default_profile: "0".into(),
        models: (0..50)
            .map(|n| ModelInfo {
                capabilities: Default::default(),
                profile: n.to_string(),
                provider: "fixture".into(),
                model: format!("model-{n:02}"),
                revision: 1,
                context_window_tokens: 64000,
                sliding_window_tokens: 38400,
                output_tokens: 2048,
            })
            .collect(),
    });
    state.open_panel(Panel::Model);
    state.model_selected = 49;
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    let last = render(&mut terminal, &mut view, &mut state);
    assert!(last.contains("> model-49") && !last.contains("model-00"));
    state.open_panel(Panel::Help);
    render(&mut terminal, &mut view, &mut state);
    let scroll = state.scroll;
    app::handle_input(
        Input::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: view.panel.x + 1,
            row: view.panel.y + 1,
            modifiers: KeyModifiers::NONE,
        }),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert_eq!(state.help_scroll, 3);
    assert_eq!(state.scroll, scroll);
    let scrolled = render(&mut terminal, &mut view, &mut state);
    assert!(scrolled.contains("Ctrl+V"));
    assert_eq!(state.scroll, scroll);
}

fn builtin_skill(name: &str) -> kyler_protocol::api::skills::SkillInfo {
    use kyler_protocol::api::skills::{SkillInfo, SkillSource, SkillStatus};

    SkillInfo {
        id: name.into(),
        name: name.into(),
        description: "Built-in guidance".into(),
        source: SkillSource::Builtin,
        path: format!("builtin://{name}"),
        fingerprint: "fixture".into(),
        status: SkillStatus::Enabled,
        resources: vec![],
        error: None,
    }
}

#[tokio::test]
async fn skills_actions_prepare_explicit_drafts_without_starting_a_run() {
    let client = Client::new(ClientSettings::default()).unwrap();
    let mut state = ready();
    state.skills = vec![
        builtin_skill("skill-creator"),
        builtin_skill("skill-installer"),
    ];
    let mut effects = Effects::default();
    let (send, _receive) = tokio::sync::mpsc::channel(16);
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    for (index, name) in [(0, "skill-creator"), (1, "skill-installer")] {
        state.open_panel(crate::state::Panel::Skills);
        state.skills_selected = index;
        let panel = render(&mut terminal, &mut view, &mut state);
        assert!(panel.contains("Create skill…"));
        assert!(panel.contains("Install skill…"));
        assert!(panel.contains("Built-in"));
        assert_eq!(view.panel.bottom(), view.input.y);
        app::handle_input(
            Input::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            &mut state,
            &mut effects,
            &client,
            &send,
            &view,
        );
        assert!(!state.skills_open);
        assert_eq!(state.composer.lines().join("\n"), format!("${name} "));
        assert!(state.pending.is_none());
        assert!(state.entries.is_empty());
        state.composer = crate::components::composer::new();
    }
}

#[tokio::test]
async fn dollar_completion_only_inserts_enabled_skills_and_preserves_draft_prefix() {
    use kyler_protocol::api::skills::SkillStatus;

    let client = Client::new(ClientSettings::default()).unwrap();
    let mut state = ready();
    let mut disabled = builtin_skill("skill-disabled");
    disabled.status = SkillStatus::Disabled;
    state.skills = vec![
        builtin_skill("skill-creator"),
        builtin_skill("skill-installer"),
        disabled,
    ];
    state.composer.insert_str("Please use $skill-c");
    let mut effects = Effects::default();
    let (send, _receive) = tokio::sync::mpsc::channel(16);
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let screen = render(&mut terminal, &mut view, &mut state);
    assert!(screen.contains("$skill-creator"));
    assert!(!screen.contains("$skill-disabled"));
    assert_eq!(view.panel.bottom(), view.input.y);
    app::handle_input(
        Input::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert_eq!(
        state.composer.lines().join("\n"),
        "Please use $skill-creator "
    );
    assert!(state.pending.is_none());
    state.composer = crate::components::composer::new();
    state.composer.insert_str("$");
    app::handle_input(
        Input::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(crate::components::skill_picker::candidates(&state).is_empty());
    assert_eq!(state.composer.lines().join("\n"), "$");
}
