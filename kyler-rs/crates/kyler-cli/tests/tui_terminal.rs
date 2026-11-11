#![cfg(unix)]
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{Json, Router, body::Body, extract::State, response::Response, routing::post};
use serde_json::{Value, json};
use tokio::{net::TcpListener, process::Command};

async fn completion(
    State(inputs): State<Arc<Mutex<Vec<String>>>>,
    Json(body): Json<Value>,
) -> Response {
    let input = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|m| m["role"] == "user")
        .unwrap()["content"]
        .as_str()
        .unwrap()
        .to_owned();
    inputs.lock().unwrap().push(input);
    let reasoning = json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"fixture-model","choices":[{"index":0,"delta":{"reasoning_content":"Inspecting streamed reasoning"},"finish_reason":null}]});
    let chunk = json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"fixture-model","choices":[{"index":0,"delta":{"content":"Hello from TUI"},"finish_reason":null}]});
    let end = json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"fixture-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(async_stream::stream! {
            yield Ok::<_, std::convert::Infallible>(format!("data: {reasoning}\n\n"));
            tokio::time::sleep(Duration::from_secs(1)).await;
            yield Ok(format!("data: {chunk}\n\ndata: {end}\n\ndata: [DONE]\n\n"));
        }))
        .unwrap()
}
#[tokio::test]
async fn actual_tui_accepts_chinese_paste_streams_and_restores_terminal_on_exit() {
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/v1/chat/completions", post(completion))
        .with_state(inputs.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("kyler.toml");
    std::fs::write(
        &config,
        format!(
            r#"
[llm]
base_url = "{url}/v1"
model = "fixture-model"
api_key = "fixture-only"
[session]
enabled = true
directory = "sessions"
[sandbox]
mode = "external-sandbox"
external_sandbox_confirmed = true
"#
        ),
    )
    .unwrap();
    let script = r#"
import os, pty, termios, fcntl, struct, subprocess, select, time, signal, sys
master, slave=pty.openpty()
fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',28,100,0,0))
before=termios.tcgetattr(slave)
env={k:v for k,v in os.environ.items() if not k.startswith('KYLER_') and k!='OPENAI_API_KEY'}
env['TERM']='xterm-256color';env['HOME']=os.path.dirname(sys.argv[2])
p=subprocess.Popen([sys.argv[1],'--config',sys.argv[2],'tui'],stdin=slave,stdout=slave,stderr=slave,env=env)
output=b''
def wait_for(marker, start=0):
    global output
    until=time.monotonic()+10
    while time.monotonic()<until:
        if marker in output[start:]:return
        if select.select([master],[],[],0.1)[0]:output+=os.read(master,65536)
        if p.poll() is not None:break
    raise AssertionError('terminal marker not found: '+repr(marker)+'; tail='+repr(output[-2000:]))
try:
    wait_for(b'fixture-model')
    os.write(master,b'\x1b[200~'+'\u4f60\u597d\n\u7b2c\u4e8c\u884c'.encode()+b'\x1b[201~')
    time.sleep(0.15)
    os.write(master,b'\r')
    wait_for(b'Inspecting')
    assert b'Hello' not in output,'reasoning was buffered until the answer'
    wait_for(b'Hello')
    wait_for(b'from')
    wait_for(b'TUI')
    wait_for(b'Worked for')
    offset=len(output)
    os.write(master,b'\x0f')
    wait_for(b'Inspecting',offset)
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
        vec!["\u{4f60}\u{597d}\n\u{7b2c}\u{4e8c}\u{884c}".to_owned()]
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
