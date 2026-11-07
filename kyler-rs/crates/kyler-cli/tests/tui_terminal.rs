#![cfg(unix)]
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    response::Response,
    routing::{get, post},
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, process::Command};

const SESSION: &str = "00000000-0000-4000-8000-000000000001";
const RUN: &str = "00000000-0000-4000-8000-000000000002";
async fn accept(
    State(inputs): State<Arc<Mutex<Vec<String>>>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    inputs
        .lock()
        .unwrap()
        .push(body["input"].as_str().unwrap().into());
    Json(json!({"run_id":RUN,"session_id":SESSION,"status":"running","events_url":"unused"}))
}
async fn create_session(
    State(inputs): State<Arc<Mutex<Vec<String>>>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let workspace = body["workspace_root"].as_str().unwrap().to_owned();
    inputs
        .lock()
        .unwrap()
        .push(format!("workspace:{workspace}"));
    Json(
        json!({"session_id":SESSION,"created_at":0,"updated_at":0,"event_count":0,"execution_count":0,"status":null,"workspace_root":workspace,"working_directory":workspace}),
    )
}
async fn events() -> Response {
    let first = json!({"run_id":RUN,"execution_id":null,"seq":1,"timestamp_ms":0,"type":"text_delta","step":1,"text":"Hello from TUI"});
    let last = json!({"run_id":RUN,"execution_id":null,"seq":2,"timestamp_ms":0,"type":"run_completed","output":"Hello from TUI","output_truncated":false,"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}});
    Response::builder().header("content-type","text/event-stream").body(Body::from(format!("id: 1\nevent: text_delta\ndata: {first}\n\nid: 2\nevent: run_completed\ndata: {last}\n\n"))).unwrap()
}
#[tokio::test]
async fn actual_tui_accepts_chinese_paste_streams_and_restores_terminal_on_exit() {
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let app=Router::new()
        .route("/v1/info",get(||async {Json(json!({"model":"fixture-model","context_window_tokens":128000,"sliding_window_tokens":76800,"workspace_root":"/project","working_directory":"/project"}))}))
        .route("/v1/sessions",post(create_session))
        .route("/v1/sessions/{id}/usage",get(||async {Json(json!({"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}))}))
        .route("/v1/sessions/{id}/history",get(||async {Json(json!({"events":[],"executions":[],"next_cursor":null}))}))
        .route("/v1/sessions/{id}/runs",post(accept))
        .route("/v1/runs/{id}",get(||async {Json(json!({"run_id":RUN,"session_id":SESSION,"execution_id":null,"status":"running","created_at":0,"finished_at":null,"last_event_seq":2,"output":null,"usage":null,"error":null}))}))
        .route("/v1/runs/{id}/events",get(events)).with_state(inputs.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("kyler.toml");
    std::fs::write(&config, "").unwrap();
    let script = r#"
import os, pty, termios, fcntl, struct, subprocess, select, time, signal, sys
master, slave=pty.openpty()
fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',28,100,0,0))
before=termios.tcgetattr(slave)
env=os.environ.copy();env['TERM']='xterm-256color';env.pop('KYLER_INTERNAL_TOOL_WORKER',None)
p=subprocess.Popen([sys.argv[1],'--config',sys.argv[2],'--server',sys.argv[3],'tui'],stdin=slave,stdout=slave,stderr=slave,env=env)
output=b''
def wait_for(marker):
    global output
    until=time.monotonic()+10
    while time.monotonic()<until:
        if marker in output:return
        if select.select([master],[],[],0.1)[0]:output+=os.read(master,65536)
        if p.poll() is not None:break
    raise AssertionError('terminal marker not found: '+repr(marker)+'; tail='+repr(output[-2000:]))
try:
    wait_for(b'fixture-model')
    os.write(master,b'\x1b[200~'+'\u4f60\u597d\n\u7b2c\u4e8c\u884c'.encode()+b'\x1b[201~')
    time.sleep(0.15)
    os.write(master,b'\r')
    wait_for(b'Hello')
    wait_for(b'from')
    wait_for(b'TUI')
    fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',24,80,0,0));os.kill(p.pid,signal.SIGWINCH)
    time.sleep(0.2)
    os.write(master,b'\x03')
    until=time.monotonic()+5
    while p.poll() is None and time.monotonic()<until:
        if select.select([master],[],[],0.05)[0]:output+=os.read(master,65536)
    assert p.poll()==0,'TUI did not exit; tail='+repr(output[-2000:])
    while select.select([master],[],[],0.05)[0]:output+=os.read(master,65536)
    assert termios.tcgetattr(slave)==before,'terminal mode was not restored'
    assert b'\x1b[?1049l' in output,'alternate screen was not restored'
    assert b'\x1b[?2004l' in output,'bracketed paste was not disabled'
finally:
    if p.poll() is None:p.kill();p.wait()
    os.close(master);os.close(slave)
"#;
    let output = tokio::time::timeout(
        Duration::from_secs(25),
        Command::new("python3")
            .arg("-c")
            .arg(script)
            .arg(env!("CARGO_BIN_EXE_kyler"))
            .arg(&config)
            .arg(&url)
            .current_dir(dir.path())
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
    assert_eq!(
        *inputs.lock().unwrap(),
        vec![
            format!("workspace:{}", dir.path().canonicalize().unwrap().display()),
            "\u{4f60}\u{597d}\n\u{7b2c}\u{4e8c}\u{884c}".into()
        ]
    );
    let output = Command::new(env!("CARGO_BIN_EXE_kyler"))
        .arg("--config")
        .arg(&config)
        .arg("tui")
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive terminal"));
    server.abort();
}
