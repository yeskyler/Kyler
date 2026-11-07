mod attachments;
mod panels;
mod rendering;
mod streaming;

use crate::{
    layout::View,
    state::{Phase, State},
};

use ratatui::{Terminal, backend::TestBackend};

use kyler_client::ClientEvent;
use kyler_protocol::api::{
    ApiErrorBody, ServerInfo, SessionUsage,
    event::{EventPayload, ServerEvent},
    run::{RunInfo, RunStatus},
};

fn ready() -> State {
    let mut state = State::new(Some("00000000-0000-4000-8000-000000000001".into()));
    state.phase = Phase::Ready;
    state.info = Some(ServerInfo {
        default_workspace: None,
        instance_id: None,
        model: "DeepSeek V4 Flash".into(),
        context_window_tokens: 128000,
        sliding_window_tokens: Some(76800),
        workspace_root: Some("/project".into()),
        working_directory: Some("/project".into()),
    });
    state.usage = Some(SessionUsage {
        total_tokens: 8932,
        ..Default::default()
    });
    state
}
fn render(terminal: &mut Terminal<TestBackend>, view: &mut View, state: &mut State) -> String {
    terminal.draw(|frame| view.render(frame, state)).unwrap();
    let buffer = terminal.backend().buffer();
    use unicode_width::UnicodeWidthStr;

    (0..buffer.area.height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0;
            while x < buffer.area.width {
                let symbol = buffer[(x, y)].symbol();
                line.push_str(symbol);
                x += symbol.width().max(1) as u16;
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn server(state: &mut State, seq: u64, payload: EventPayload) {
    state.stream(ClientEvent::Server(ServerEvent {
        run_id: "run".into(),
        execution_id: None,
        seq,
        timestamp_ms: 0,
        payload,
    }));
}
fn result(answer: &str) -> ClientEvent {
    ClientEvent::Result(RunInfo {
        permissions: None,
        model: None,
        run_id: "run".into(),
        session_id: "session".into(),
        execution_id: None,
        status: RunStatus::Completed,
        created_at: 0,
        finished_at: Some(0),
        last_event_seq: 4,
        output: Some(answer.into()),
        usage: None,
        error: None,
    })
}

fn model_failure() -> ClientEvent {
    let ClientEvent::Result(mut info) = result("") else {
        unreachable!()
    };
    info.status = RunStatus::Failed;
    info.output = None;
    info.error = Some(ApiErrorBody {
        code: "model_image_unsupported".into(),
        message: "The selected model does not support image input. Use /model to select a vision-capable model, then resend.".into(),
    });
    ClientEvent::Result(info)
}
