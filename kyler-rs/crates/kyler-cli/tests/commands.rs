use std::{
    path::PathBuf,
    process::{Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use tokio::{io::AsyncWriteExt, net::TcpListener, process::Command, task::JoinHandle};

use kyler_protocol::api::{
    ApiErrorBody,
    event::{EventPayload, ServerEvent},
    run::{RunInfo, RunStatus, Usage},
    session::SessionSummary,
};

const RUN: &str = "00000000-0000-4000-8000-000000000001";
const SESSION: &str = "00000000-0000-4000-8000-000000000002";
#[derive(Default)]
struct StateData {
    failed: bool,
    hold: bool,
    cancelled: AtomicBool,
    subscribed: AtomicBool,
    starts: AtomicUsize,
}
struct Fixture {
    url: String,
    config: PathBuf,
    _dir: tempfile::TempDir,
    task: JoinHandle<()>,
    state: Arc<StateData>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new(state: StateData) -> Self {
        let state = Arc::new(state);
        let app = Router::new()
            .route("/v1/sessions", post(session))
            .route("/v1/sessions/{id}", get(session))
            .route("/v1/sessions/{id}/runs", post(start))
            .route("/v1/runs/{id}", get(status))
            .route("/v1/runs/{id}/events", get(events))
            .route("/v1/runs/{id}/cancel", post(cancel))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("kyler.toml");
        std::fs::write(
            &config,
            "[llm]\napi_key=''\n[client]\nreconnect_delay_ms=1\ncancel_timeout_seconds=2\n",
        )
        .unwrap();
        Self {
            url,
            config,
            _dir: dir,
            task,
            state,
        }
    }
    fn cmd(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kyler"));
        cmd.arg("--config")
            .arg(&self.config)
            .arg("--server")
            .arg(&self.url)
            .current_dir(self._dir.path())
            .env_remove("KYLER_LLM_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        cmd
    }
    async fn output(&self, args: &[&str]) -> Output {
        tokio::time::timeout(Duration::from_secs(10), self.cmd().args(args).output())
            .await
            .unwrap()
            .unwrap()
    }
}
async fn session() -> Json<SessionSummary> {
    Json(SessionSummary {
        title: None,
        draft: false,
        workspace_root: None,
        working_directory: None,
        session_id: SESSION.into(),
        created_at: 0,
        updated_at: 0,
        event_count: 0,
        execution_count: 0,
        status: None,
    })
}
async fn start(State(state): State<Arc<StateData>>) -> impl IntoResponse {
    state.starts.fetch_add(1, Ordering::SeqCst);
    (
        StatusCode::ACCEPTED,
        Json(
            serde_json::json!({"run_id":RUN,"session_id":SESSION,"status":"running","events_url":format!("/v1/runs/{RUN}/events")}),
        ),
    )
}
fn info(state: &StateData) -> RunInfo {
    let status = if state.cancelled.load(Ordering::SeqCst) {
        RunStatus::Cancelled
    } else if state.failed {
        RunStatus::Failed
    } else {
        RunStatus::Running
    };
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
        output: None,
        usage: None,
        error: state.failed.then(|| ApiErrorBody {
            code: "model_failed".into(),
            message: "Model unavailable".into(),
        }),
    }
}
async fn status(State(state): State<Arc<StateData>>) -> Json<RunInfo> {
    Json(info(&state))
}
async fn cancel(State(state): State<Arc<StateData>>) -> Json<RunInfo> {
    state.cancelled.store(true, Ordering::SeqCst);
    Json(info(&state))
}
fn frame(seq: u64, payload: EventPayload) -> String {
    let name = payload.name();
    format!(
        "id: {seq}\nevent: {name}\ndata: {}\n\n",
        serde_json::to_string(&ServerEvent {
            run_id: RUN.into(),
            execution_id: None,
            seq,
            timestamp_ms: 0,
            payload
        })
        .unwrap()
    )
}
async fn events(State(state): State<Arc<StateData>>) -> Response {
    let stream = async_stream::stream! {
        yield Ok::<_,std::io::Error>(frame(1,EventPayload::RunStarted {model:None,session_id:SESSION.into()}));
        yield Ok(frame(2,EventPayload::TextDelta {step:1,text:"Hello".into()}));
        state.subscribed.store(true,Ordering::SeqCst);
        if state.hold {while !state.cancelled.load(Ordering::SeqCst) {tokio::time::sleep(Duration::from_millis(10)).await;}}
        let terminal=if state.cancelled.load(Ordering::SeqCst) {EventPayload::RunCancelled}
            else if state.failed {EventPayload::RunFailed {error:info(&state).error.unwrap()}}
            else {EventPayload::RunCompleted {output:"Hello".into(),output_truncated:false,usage:Usage::default()}};
        yield Ok(frame(3,terminal));
    };
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
        .into_response()
}
#[tokio::test]
async fn plain_and_final_only_output_do_not_duplicate_final_text_or_require_model_keys() {
    let fixture = Fixture::new(StateData::default()).await;
    for args in [vec!["run", "hi"], vec!["run", "--final-only", "hi"]] {
        let out = fixture.output(&args).await;
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "Hello\n");
        assert!(
            String::from_utf8(out.stderr)
                .unwrap()
                .contains(&format!("Run: {RUN}"))
        );
    }
    assert_eq!(fixture.state.starts.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn json_stdout_contains_only_json_records_and_full_final_snapshot() {
    let fixture = Fixture::new(StateData::default()).await;
    let out = fixture.output(&["--json", "run", "hi"]).await;
    assert!(out.status.success());
    let records: Vec<serde_json::Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(records[0]["type"], "run_started");
    assert_eq!(records.last().unwrap()["type"], "client_result");
    assert_eq!(records.last().unwrap()["run"]["output"], "Hello");
}
#[tokio::test]
async fn chat_accepts_piped_turns_and_exits_on_command_or_eof() {
    let fixture = Fixture::new(StateData::default()).await;
    let mut cmd = fixture.cmd();
    cmd.stdin(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/session\nhi\n/exit\n")
        .await
        .unwrap();
    let out = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, b"Hello\n");
    let eof = fixture.output(&["chat"]).await;
    assert!(eof.status.success());
    assert!(eof.stdout.is_empty());
}
#[tokio::test]
async fn run_failure_and_invalid_parameters_have_distinct_exit_codes() {
    let fixture = Fixture::new(StateData {
        failed: true,
        ..Default::default()
    })
    .await;
    let out = fixture.output(&["run", "--final-only", "hi"]).await;
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let out = fixture.output(&["runs", "show", "bad-id"]).await;
    assert_eq!(out.status.code(), Some(2));
    let out = fixture
        .output(&["--json", "run", "--final-only", "hi"])
        .await;
    assert_eq!(out.status.code(), Some(2));
}
#[cfg(unix)]
#[tokio::test]
async fn ctrl_c_requests_cancellation_and_waits_for_confirmation() {
    let fixture = Fixture::new(StateData {
        hold: true,
        ..Default::default()
    })
    .await;
    let child = fixture.cmd().args(["run", "hi"]).spawn().unwrap();
    let pid = child.id().unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !fixture.state.subscribed.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        Command::new("/bin/kill")
            .args(["-INT", &pid.to_string()])
            .status()
            .await
            .unwrap()
            .success()
    );
    let out = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(fixture.state.cancelled.load(Ordering::SeqCst));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cancelled"));
}

#[tokio::test]
async fn client_uses_user_config_and_ignores_project_config_and_dotenv() {
    let fixture = Fixture::new(StateData::default()).await;
    let home = fixture._dir.path().join("home");
    let global = home.join(".kyler");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::write(
        global.join("config.toml"),
        format!("[client]\nserver_url={:?}\n", fixture.url),
    )
    .unwrap();
    let project = fixture._dir.path().join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("kyler.toml"), "invalid TOML").unwrap();
    std::fs::write(
        project.join(".env"),
        "KYLER_CLIENT_SERVER_URL=http://127.0.0.1:1\n",
    )
    .unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(env!("CARGO_BIN_EXE_kyler"))
            .args(["run", "--final-only", "hi"])
            .env("HOME", &home)
            .env_remove("KYLER_CONFIG")
            .env_remove("KYLER_CLIENT_SERVER_URL")
            .env_remove("KYLER_SERVER_TOKEN")
            .env_remove("KYLER_CLIENT_TOKEN")
            .current_dir(project)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"Hello\n");
}

#[tokio::test]
async fn eval_runs_a_task_suite_and_writes_a_machine_readable_report() {
    let fixture = Fixture::new(StateData::default()).await;
    let suite = fixture._dir.path().join("suite.json");
    let report = fixture._dir.path().join("report.json");
    std::fs::write(&suite,r#"[{"name":"greeting","input":"hello","answer_contains":["Hello"],"max_tokens":100,"timeout_seconds":5}]"#).unwrap();
    let output = fixture
        .output(&[
            "eval",
            "--suite",
            suite.to_str().unwrap(),
            "--report",
            report.to_str().unwrap(),
        ])
        .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
    assert_eq!(json["passed"], 1);
    assert_eq!(json["cases"][0]["passed"], true);
}
