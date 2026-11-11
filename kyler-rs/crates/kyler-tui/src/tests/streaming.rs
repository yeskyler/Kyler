use super::{ready, render, result, server};
use crate::agent::{AgentEvent, AgentHandle};
use crate::{
    app,
    components::{status, transcript},
    effects::{Effects, Event},
    layout::View,
    state::{Entry, Phase},
};

use std::time::Instant;

use ratatui::{Terminal, backend::TestBackend};

use kyler_protocol::api::{SessionUsage, event::EventPayload, run::RunStatus};

#[test]
fn working_stays_above_composer_when_reviewing_history() {
    let mut state = ready();
    state.phase = Phase::Running;
    state.entries.push(Entry::Message {
        role: "assistant".into(),
        content: "history\n".repeat(100),
    });
    let mut view = View::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    for follow in [true, false] {
        state.follow = follow;
        state.scroll = 0;
        let screen = render(&mut terminal, &mut view, &mut state);
        let rows: Vec<_> = screen.lines().collect();
        assert!(rows[usize::from(view.input.y - 1)].contains("working · 0s"));
        assert!(!screen.contains("Sliding window"));
        assert!(!screen.contains("Maximum window"));
        assert_eq!(view.input.bottom(), 28);
    }
}

#[test]
fn reviewing_history_does_not_jump_on_stream_updates_and_bottom_resumes_following() {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let mut view = View::default();
    let mut state = ready();
    state.entries.push(Entry::Message {
        role: "assistant".into(),
        content: "history\n".repeat(100),
    });
    render(&mut terminal, &mut view, &mut state);
    state.scroll_by(-10);
    let scroll = state.scroll;
    assert!(!state.follow);
    state.notice("new output");
    render(&mut terminal, &mut view, &mut state);
    assert_eq!(state.scroll, scroll);
    state.scroll_by(isize::MAX);
    assert!(state.follow);
    render(&mut terminal, &mut view, &mut state);
    assert_eq!(state.scroll, state.max_scroll);
}

#[test]
fn final_snapshot_replaces_streamed_answer_once() {
    let mut state = ready();
    state.run = Some("run".into());
    state.phase = Phase::Running;
    server(
        &mut state,
        1,
        EventPayload::TextDelta {
            step: 1,
            text: "Hel".into(),
        },
    );
    server(
        &mut state,
        1,
        EventPayload::TextDelta {
            step: 1,
            text: "duplicate".into(),
        },
    );
    server(
        &mut state,
        2,
        EventPayload::TextDelta {
            step: 1,
            text: "lo".into(),
        },
    );
    state.stream(result("Hello"));
    assert_eq!(state.entries.len(), 2);
    assert!(
        matches!(state.entries.last(), Some(Entry::RunTiming { label, .. }) if label == "Worked for")
    );
    state.stream(result("Hello"));
    assert_eq!(state.entries.len(), 2);
    assert!(matches!(&state.entries[0],Entry::Assistant(cell) if cell.source()=="Hello"));
    assert_eq!(state.phase, Phase::Ready);
}

#[test]
fn usage_is_authoritative_monotone_and_old_generation_results_are_ignored() {
    let client = AgentHandle::new(Default::default(), None);
    let mut effects = Effects::default();
    let (send, _) = tokio::sync::mpsc::channel(8);
    let mut state = ready();
    state.generation = 2;
    app::apply(
        1,
        Event::Usage(SessionUsage {
            total_tokens: 99999,
            ..Default::default()
        }),
        &mut state,
        &mut effects,
        &client,
        &send,
    );
    app::apply(
        2,
        Event::Usage(SessionUsage {
            total_tokens: 10,
            ..Default::default()
        }),
        &mut state,
        &mut effects,
        &client,
        &send,
    );
    assert_eq!(state.usage.as_ref().unwrap().total_tokens, 8932);
    assert_eq!(
        status::text(&state),
        "Read-only · Workspace access · Full access"
    );
}

#[test]
fn tool_activity_stream_updates_rendered_status_and_output_size() {
    use kyler_protocol::activity::ActivityKind;

    let mut state = ready();
    state.run = Some("run".into());
    server(
        &mut state,
        1,
        EventPayload::ToolStarted {
            summary: Some("src/runtime.rs".into()),
            call_id: "call".into(),
            name: "read_file".into(),
            activity: ActivityKind::Explore,
            source: Default::default(),
        },
    );
    let before = transcript::fingerprint(state.entries.last().unwrap());
    server(
        &mut state,
        2,
        EventPayload::ToolFinished {
            error: None,
            call_id: "call".into(),
            name: "read_file".into(),
            status: "success".into(),
            output_bytes: 1234,
        },
    );
    assert_ne!(
        before,
        transcript::fingerprint(state.entries.last().unwrap())
    );
    let text = transcript::lines(&state.entries)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("✓ Explore · 1 call"));
    assert!(text.contains("└─ ✓ read_file"));
    assert!(text.contains("1.2 KB"));
    assert!(text.contains("src/runtime.rs"));
    server(
        &mut state,
        3,
        EventPayload::ToolFinished {
            call_id: "call".into(),
            name: "read_file".into(),
            status: "error".into(),
            output_bytes: 20,
            error: Some("File was not found".into()),
        },
    );
    let rows = transcript::tool_rows(&state.entries, 80);
    let text = rows
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(text.matches("File was not found").count(), 1);
    assert!(text.contains("Failed · F3 details"));
    let legacy_finished: EventPayload = serde_json::from_value(serde_json::json!({
        "type":"tool_finished","call_id":"old","name":"custom","status":"error","output_bytes":0
    }))
    .unwrap();
    assert!(matches!(
        legacy_finished,
        EventPayload::ToolFinished { error: None, .. }
    ));
    let legacy: EventPayload = serde_json::from_value(
        serde_json::json!({"type":"tool_started","call_id":"old","name":"custom"}),
    )
    .unwrap();
    assert!(matches!(
        legacy,
        EventPayload::ToolStarted {
            summary: None,
            activity: ActivityKind::Think,
            ..
        }
    ));
}

#[test]
fn working_animation_advances_and_completion_duration_is_below_answer() {
    let mut state = ready();
    state.run_started = Some(Instant::now() - std::time::Duration::from_millis(65_200));
    assert!(status::working(&state).contains("working · 1m 05s"));
    state.phase = Phase::Running;
    state.run = Some("run".into());
    let mut event = result("Final answer");
    if let AgentEvent::Result(info) = &mut event {
        info.created_at = 1000;
        info.finished_at = Some(76_000);
    }
    state.stream(event);
    assert!(state.run_started.is_none());
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let screen = render(&mut terminal, &mut View::default(), &mut state);
    assert!(screen.find("Final answer").unwrap() < screen.find("Worked for 1m 15s").unwrap());
    assert!(!screen.contains("working ·"));
    assert_eq!(status::duration(3661), "1h 01m 01s");
}

#[test]
fn assistant_deltas_wait_for_ticks_and_keep_markdown_context_across_packets() {
    let mut state = ready();
    state.run = Some("run".into());
    state.phase = Phase::Running;
    server(
        &mut state,
        1,
        EventPayload::TextDelta {
            step: 1,
            text: "A **bold".into(),
        },
    );
    let Entry::Assistant(cell) = &state.entries[0] else {
        panic!("AI messages need their own cell");
    };
    assert_eq!(cell.source(), "A **bold");
    assert_eq!(
        cell.displayed(),
        "",
        "Receiving a packet must not commit terminal rows"
    );
    server(
        &mut state,
        2,
        EventPayload::TextDelta {
            step: 1,
            text: " example**.\n".into(),
        },
    );
    state.advance_messages();
    let rows = transcript::lines_with_width(&state.entries, 80);
    let bold = rows
        .iter()
        .flat_map(|row| &row.spans)
        .filter(|span| {
            span.style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
        })
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(bold, "bold example");
    assert_eq!(state.entries.len(), 1);
    state.stream(result("A **bold example**."));
    assert_eq!(
        state
            .entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Assistant(_)))
            .count(),
        1
    );
}

#[test]
fn tool_boundaries_flush_partial_ai_text_before_the_activity_and_final_reply() {
    let mut state = ready();
    state.phase = Phase::Running;
    state.run = Some("run".into());
    server(
        &mut state,
        1,
        EventPayload::TextDelta {
            step: 1,
            text: "Checking the files.".into(),
        },
    );
    server(
        &mut state,
        2,
        EventPayload::ToolStarted {
            call_id: "read".into(),
            name: "read_file".into(),
            summary: Some("src/lib.rs".into()),
            activity: Default::default(),
            source: Default::default(),
        },
    );
    assert!(
        matches!(&state.entries[0], Entry::Assistant(cell) if cell.is_finished() && cell.displayed() == "Checking the files.")
    );
    assert!(matches!(&state.entries[1], Entry::Tool { .. }));
    server(&mut state, 3, EventPayload::StepStarted { step: 2 });
    server(
        &mut state,
        4,
        EventPayload::TextDelta {
            step: 2,
            text: "Final answer.".into(),
        },
    );
    state.stream(result("Final answer."));
    let text = transcript::lines_with_width(&state.entries, 80)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.find("Checking the files.").unwrap() < text.find("read_file").unwrap());
    assert!(text.find("read_file").unwrap() < text.find("Final answer.").unwrap());
    assert_eq!(text.matches("Final answer.").count(), 1);
    assert!(text.contains("────"));
}

#[test]
fn stream_queue_is_bounded_per_tick_and_finish_preserves_unicode_and_tail() {
    use crate::components::messages::assistant::AgentMessageCell;

    let mut cell = AgentMessageCell::default();
    let source = "line\n".repeat(40) + &"\u{4e2d}🙂e\u{301}".repeat(200);
    cell.push(&source);
    cell.advance();
    assert_eq!(cell.displayed(), "line\n".repeat(4));
    assert!(cell.displayed().len() < cell.source().len());
    cell.finish(None);
    assert_eq!(cell.displayed(), source);
    assert!(!cell.advance());
}

#[test]
fn cancellation_flushes_ai_tail_and_final_recovery_reuses_the_current_cell() {
    let mut state = ready();
    state.phase = Phase::Running;
    state.run = Some("run".into());
    server(
        &mut state,
        1,
        EventPayload::TextDelta {
            step: 1,
            text: "Partial tail".into(),
        },
    );
    state.stream(result("Recovered full answer"));
    assert_eq!(
        state
            .entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Assistant(_)))
            .count(),
        1
    );
    assert!(
        matches!(&state.entries[0], Entry::Assistant(cell) if cell.displayed() == "Recovered full answer")
    );
    state.phase = Phase::Running;
    server(&mut state, 2, EventPayload::StepStarted { step: 2 });
    server(
        &mut state,
        3,
        EventPayload::TextDelta {
            step: 2,
            text: "Cancelled tail".into(),
        },
    );
    let mut done = match result("") {
        AgentEvent::Result(done) => done,
        _ => unreachable!(),
    };
    done.output = None;
    done.status = RunStatus::Cancelled;
    state.stream(AgentEvent::Result(done));
    assert!(state.entries.iter().any(|entry| matches!(entry, Entry::Assistant(cell) if cell.is_finished() && cell.displayed() == "Cancelled tail")));
}

#[test]
fn queued_batch_nodes_are_updated_in_place_and_cancelled_with_the_run() {
    let mut state = ready();
    state.run = Some("run".into());
    state.phase = Phase::Running;
    for (seq, id) in [(1, "a"), (2, "b")] {
        server(
            &mut state,
            seq,
            EventPayload::ToolQueued {
                call_id: id.into(),
                name: "read_file".into(),
                summary: None,
                activity: Default::default(),
                source: Default::default(),
            },
        );
    }
    assert_eq!(
        state
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Tool { status, .. } if status == "queued"))
            .count(),
        2
    );
    server(
        &mut state,
        3,
        EventPayload::ToolStarted {
            call_id: "a".into(),
            name: "read_file".into(),
            summary: None,
            activity: Default::default(),
            source: Default::default(),
        },
    );
    assert_eq!(
        state
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Tool { .. }))
            .count(),
        2
    );
    let text = transcript::lines(&state.entries)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Queued"));
    let AgentEvent::Result(mut info) = result("") else {
        unreachable!()
    };
    info.status = RunStatus::Cancelled;
    state.stream(AgentEvent::Result(info));
    assert_eq!(
        state
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Tool { status, .. } if status == "cancelled"))
            .count(),
        2
    );
}

#[test]
fn compaction_updates_one_separator_and_stops_when_cancelled() {
    use kyler_protocol::api::compaction::{CompactionInfo, CompactionStatus};
    let mut state = ready();
    state.phase = Phase::Running;
    state.run = Some("run".into());
    let mut info = CompactionInfo {
        id: "compaction".into(),
        status: CompactionStatus::Running,
        started_at: 1,
        finished_at: None,
    };
    server(
        &mut state,
        1,
        EventPayload::ContextCompaction {
            compaction: info.clone(),
        },
    );
    assert!(state.entries.iter().any(|entry| matches!(entry, Entry::Compaction(record) if record.status == CompactionStatus::Running)));
    info.status = CompactionStatus::Completed;
    info.finished_at = Some(2);
    server(
        &mut state,
        2,
        EventPayload::ContextCompaction { compaction: info },
    );
    assert_eq!(
        state
            .entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Compaction(_)))
            .count(),
        1
    );
    let lines = transcript::lines_with_width(&state.entries, 80);
    assert!(format!("{lines:?}").contains("Context compacted"));
}

#[test]
fn preparing_tool_is_updated_in_place_and_settled_on_cancel() {
    let mut state = ready();
    state.run = Some("run".into());
    state.phase = Phase::Running;
    server(
        &mut state,
        1,
        EventPayload::ToolCallDelta {
            call_id: "a".into(),
            name: "read".into(),
        },
    );
    server(
        &mut state,
        2,
        EventPayload::ToolCallDelta {
            call_id: "a".into(),
            name: "read_file".into(),
        },
    );
    assert_eq!(
        state
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Tool { .. }))
            .count(),
        1
    );
    assert!(state.entries.iter().any(|e| matches!(e, Entry::Tool { name, status, .. } if name == "read_file" && status == "preparing")));
    let AgentEvent::Result(mut info) = result("") else {
        unreachable!()
    };
    info.status = RunStatus::Cancelled;
    state.stream(AgentEvent::Result(info));
    assert!(
        state
            .entries
            .iter()
            .any(|e| matches!(e, Entry::Tool { status, .. } if status == "cancelled"))
    );
}
