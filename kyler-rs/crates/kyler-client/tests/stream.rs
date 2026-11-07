use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use tokio::{net::TcpListener, task::JoinHandle};

use kyler_client::{Client, ClientEvent};
use kyler_config::ClientSettings;
use kyler_protocol::api::{
    event::{EventPayload, ServerEvent},
    run::{RunInfo, RunStatus, Usage},
};

const RUN: &str = "00000000-0000-4000-8000-000000000001";
const SESSION: &str = "00000000-0000-4000-8000-000000000002";
#[derive(Default)]
struct Fixture {
    cursors: Mutex<Vec<String>>,
    gap: bool,
    malformed: bool,
    reads: Mutex<usize>,
}
fn info(status: RunStatus) -> RunInfo {
    RunInfo {
        permissions: None,
        model: None,
        run_id: RUN.into(),
        session_id: SESSION.into(),
        execution_id: None,
        status,
        created_at: 0,
        finished_at: None,
        last_event_seq: 3,
        output: status.is_terminal().then(|| "\u{4f60}\u{597d}".into()),
        usage: None,
        error: None,
    }
}
fn frame(seq: u64, payload: EventPayload) -> String {
    let name = payload.name();
    let event = ServerEvent {
        run_id: RUN.into(),
        execution_id: None,
        seq,
        timestamp_ms: 0,
        payload,
    };
    format!(
        "id: {seq}\nevent: {name}\ndata: {}\n\n",
        serde_json::to_string(&event).unwrap()
    )
}
async fn status(State(state): State<Arc<Fixture>>, Path(_): Path<String>) -> Json<RunInfo> {
    let mut reads = state.reads.lock().unwrap();
    *reads += 1;
    Json(info(if state.gap && *reads > 1 {
        RunStatus::Completed
    } else {
        RunStatus::Running
    }))
}
async fn events(State(state): State<Arc<Fixture>>, headers: HeaderMap) -> Response {
    let cursor = headers
        .get("last-event-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    state.cursors.lock().unwrap().push(cursor.clone());
    if state.gap {
        return (
            StatusCode::GONE,
            Json(serde_json::json!({"code":"event_cursor_expired","message":"expired"})),
        )
            .into_response();
    }
    let data = if state.malformed {
        frame(
            7,
            EventPayload::TextDelta {
                step: 1,
                text: "bad".into(),
            },
        )
    } else if cursor == "0" {
        frame(
            1,
            EventPayload::RunStarted {
                model: None,
                session_id: SESSION.into(),
            },
        ) + &frame(
            2,
            EventPayload::TextDelta {
                step: 1,
                text: "\u{4f60}\u{597d}".into(),
            },
        )
    } else {
        frame(
            2,
            EventPayload::TextDelta {
                step: 1,
                text: "\u{4f60}\u{597d}".into(),
            },
        ) + &frame(
            3,
            EventPayload::RunCompleted {
                output: "\u{4f60}\u{597d}".into(),
                output_truncated: false,
                usage: Usage::default(),
            },
        )
    };
    let chunks: Vec<_> = data
        .as_bytes()
        .chunks(1)
        .map(|c| Ok::<_, std::io::Error>(c.to_vec()))
        .collect();
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(futures::stream::iter(chunks)))
        .unwrap()
}
async fn fixture(state: Arc<Fixture>) -> (Client, JoinHandle<()>) {
    let app = Router::new()
        .route("/v1/runs/{id}", get(status))
        .route("/v1/runs/{id}/events", get(events))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let settings = ClientSettings {
        server_url: format!("http://{}", listener.local_addr().unwrap()),
        reconnect_delay_ms: 1,
        ..Default::default()
    };
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (Client::new(settings).unwrap(), task)
}
#[tokio::test]
async fn reconnects_from_last_consumed_event_and_suppresses_duplicates() {
    let state = Arc::new(Fixture::default());
    let (client, task) = fixture(state.clone()).await;
    let mut stream = client.watch(RUN, 0).await.unwrap();
    let mut text = String::new();
    let mut final_result = false;
    let mut retries = 0;
    while let Some(event) = stream.next().await.unwrap() {
        match event {
            ClientEvent::Server(ServerEvent {
                payload: EventPayload::TextDelta { text: delta, .. },
                ..
            }) => text.push_str(&delta),
            ClientEvent::Reconnecting { .. } => retries += 1,
            ClientEvent::Result(result) => {
                assert_eq!(result.status, RunStatus::Completed);
                final_result = true;
            }
            _ => {}
        }
    }
    assert_eq!(text, "\u{4f60}\u{597d}");
    assert!(final_result);
    assert_eq!(retries, 1);
    assert_eq!(*state.cursors.lock().unwrap(), vec!["0", "2"]);
    task.abort();
}
#[tokio::test]
async fn expired_cursor_reports_gap_and_recovers_authoritative_result() {
    let state = Arc::new(Fixture {
        gap: true,
        ..Default::default()
    });
    let (client, task) = fixture(state.clone()).await;
    let mut stream = client.watch(RUN, 0).await.unwrap();
    assert!(matches!(
        stream.next().await.unwrap(),
        Some(ClientEvent::Gap { .. })
    ));
    match stream.next().await.unwrap().unwrap() {
        ClientEvent::Result(info) => assert_eq!(info.output.as_deref(), Some("\u{4f60}\u{597d}")),
        _ => panic!("expected final snapshot"),
    };
    assert!(stream.next().await.unwrap().is_none());
    assert_eq!(state.cursors.lock().unwrap().len(), 1);
    task.abort();
}
#[tokio::test]
async fn refuses_noncontiguous_output() {
    let state = Arc::new(Fixture {
        malformed: true,
        ..Default::default()
    });
    let (client, task) = fixture(state).await;
    let mut stream = client.watch(RUN, 0).await.unwrap();
    assert_eq!(
        stream.next().await.unwrap_err().kind,
        kyler_client::ErrorKind::Protocol
    );
    task.abort();
}
#[tokio::test]
async fn authorization_failure_is_not_retried() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let app=Router::new().route("/v1/sessions",post(move || {let count=count.clone();async move {count.fetch_add(1,Ordering::SeqCst);(StatusCode::UNAUTHORIZED,Json(serde_json::json!({"code":"unauthorized","message":"Authentication required"})))}}));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let settings = ClientSettings {
        server_url: format!("http://{}", listener.local_addr().unwrap()),
        ..Default::default()
    };
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let error = Client::new(settings)
        .unwrap()
        .create_session()
        .await
        .unwrap_err();
    assert_eq!(error.status, Some(401));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    task.abort();
}
