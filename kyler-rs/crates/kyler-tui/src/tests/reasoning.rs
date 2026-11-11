use super::{ready, render, result, server};
use crate::{
    agent::{AgentEvent, AgentHandle},
    app,
    components::transcript,
    effects::Effects,
    layout::View,
    state::{Entry, Phase, State},
};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{Terminal, backend::TestBackend};

use kyler_protocol::api::{
    event::EventPayload,
    history::{HistoryContent, HistoryEvent},
    run::RunStatus,
};

fn running() -> State {
    let mut state = ready();
    state.run = Some("run".into());
    state.phase = Phase::Running;
    state
}
fn text(state: &State, width: u16) -> String {
    transcript::lines_with_width(&state.entries, width)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}
fn think(state: &mut State, seq: u64, step: u32, text: &str) {
    server(
        state,
        seq,
        EventPayload::ReasoningDelta {
            step,
            text: text.into(),
        },
    );
}

#[test]
fn reasoning_streams_in_a_bounded_preview_without_losing_text() {
    let mut state = running();
    let source = (0..20)
        .map(|index| format!("line-{index:02} 中文🙂\n"))
        .collect::<String>();
    think(&mut state, 1, 1, &source);
    think(&mut state, 2, 1, "latest reasoning");
    let preview = text(&state, 50);
    assert!(preview.contains("Thinking…"));
    assert!(preview.contains("latest reasoning"));
    assert!(!preview.contains("line-00"));
    assert!(preview.lines().count() <= 8, "{preview}");
    assert_eq!(state.entries.len(), 1);
    let Entry::Reasoning(cell) = &state.entries[0] else {
        panic!("missing reasoning cell")
    };
    assert_eq!(cell.content, source + "latest reasoning");
    assert!(cell.streaming && cell.open());
}

#[test]
fn answer_collapses_reasoning_and_preserves_it_for_expansion() {
    let mut state = running();
    think(&mut state, 1, 1, "private analysis preview");
    server(
        &mut state,
        2,
        EventPayload::TextDelta {
            step: 1,
            text: "Final answer".into(),
        },
    );
    state.advance_messages();
    assert!(state.reasoning.is_none());
    let closed = text(&state, 80);
    assert!(closed.contains("Thought for"));
    assert!(!closed.contains("private analysis preview"));
    assert!(closed.contains("Final answer"));
    state.toggle_latest_reasoning();
    let expanded = text(&state, 80);
    assert!(expanded.contains("private analysis preview"));
    assert!(expanded.contains("Final answer"));
}

#[test]
fn explicit_disclosure_choice_survives_deltas_and_phase_changes() {
    let mut state = running();
    think(&mut state, 1, 1, "first thought");
    state.toggle_latest_reasoning();
    think(&mut state, 2, 1, " plus more");
    assert!(!text(&state, 80).contains("first thought"));
    state.toggle_latest_reasoning();
    server(
        &mut state,
        3,
        EventPayload::TextDelta {
            step: 1,
            text: "Answer".into(),
        },
    );
    assert!(text(&state, 80).contains("first thought plus more"));
}

#[test]
fn tool_boundaries_and_new_steps_keep_reasoning_blocks_in_order() {
    let mut state = running();
    think(&mut state, 1, 1, "first step");
    server(
        &mut state,
        2,
        EventPayload::ToolCallDelta {
            call_id: "tool".into(),
            name: "read_file".into(),
        },
    );
    think(&mut state, 3, 2, "second step");
    assert!(matches!(&state.entries[0], Entry::Reasoning(cell) if !cell.streaming && !cell.open()));
    assert!(matches!(&state.entries[1], Entry::Tool { .. }));
    assert!(matches!(&state.entries[2], Entry::Reasoning(cell) if cell.streaming && cell.open()));
}

#[test]
fn cancellation_and_failure_finish_reasoning_without_discarding_it() {
    for status in [RunStatus::Cancelled, RunStatus::Failed] {
        let mut state = running();
        think(&mut state, 1, 1, "retained partial thought");
        let AgentEvent::Result(mut info) = result("") else {
            unreachable!()
        };
        info.status = status;
        info.output = None;
        state.stream(AgentEvent::Result(info));
        assert!(state.reasoning.is_none());
        assert!(
            matches!(&state.entries[0], Entry::Reasoning(cell) if !cell.streaming && !cell.open() && cell.content == "retained partial thought")
        );
        state.toggle_latest_reasoning();
        assert!(text(&state, 80).contains("retained partial thought"));
    }
}

#[test]
fn restored_reasoning_is_collapsed_without_inventing_a_duration() {
    let mut state = ready();
    state.history(
        vec![HistoryEvent {
            id: "event".into(),
            execution_id: "execution".into(),
            timestamp_ms: 0,
            content: vec![HistoryContent::Reasoning {
                text: "saved reasoning".into(),
            }],
        }],
        vec![],
    );
    let closed = text(&state, 80);
    assert!(closed.contains("Thought"));
    assert!(!closed.contains("Thought for"));
    assert!(!closed.contains("saved reasoning"));
    state.toggle_latest_reasoning();
    assert!(text(&state, 80).contains("saved reasoning"));
}

#[test]
fn mouse_and_keyboard_toggle_reasoning_and_invalidate_the_view_cache() {
    let mut state = running();
    think(&mut state, 1, 1, "visible live thought");
    let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
    let mut view = View::default();
    let screen = render(&mut terminal, &mut view, &mut state);
    assert!(screen.contains("visible live thought"));
    let row = screen
        .lines()
        .position(|line| line.contains("Thinking…"))
        .unwrap() as u16;
    let client = AgentHandle::new(Default::default(), None);
    let mut effects = Effects::default();
    let (send, _receive) = tokio::sync::mpsc::channel(128);
    app::handle_input(
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: view.transcript.x,
            row,
            modifiers: KeyModifiers::NONE,
        }),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(!render(&mut terminal, &mut view, &mut state).contains("visible live thought"));
    state.composer.insert_str("draft stays intact");
    app::handle_input(
        Event::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
        &mut state,
        &mut effects,
        &client,
        &send,
        &view,
    );
    assert!(render(&mut terminal, &mut view, &mut state).contains("visible live thought"));
    assert_eq!(state.composer.lines().join("\n"), "draft stays intact");
    assert!(
        !state.follow,
        "manual inspection must not jump to new output"
    );
}
