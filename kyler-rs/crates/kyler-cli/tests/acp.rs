//! Actual stdio executable, temporary configuration, and loopback model only.
use std::{convert::Infallible, process::Stdio, time::Duration};

use axum::{
    Json, Router,
    response::sse::{Event, Sse},
    routing::post,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    process::Command,
};

async fn model(
    Json(request): Json<Value>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let has_result = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["role"] == "tool");
    let (delta, reason) = if has_result {
        (json!({"content":"ACP ready"}), "stop")
    } else {
        (
            json!({"tool_calls":[{"index":0,"id":"plan-call","type":"function","function":{"name":"update_plan","arguments":json!({"summary":"Test plan","steps":[{"id":"one","content":"Respond","status":"completed"}]}).to_string()}}]}),
            "tool_calls",
        )
    };
    Sse::new(futures::stream::iter(vec![
        Ok(Event::default().data(json!({"id":"fixture","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":delta,"finish_reason":reason}]}).to_string())),
        Ok(Event::default().data("[DONE]")),
    ]))
}
#[tokio::test]
async fn stdio_uses_core_and_a_private_acp_session_namespace() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let upstream = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/v1/chat/completions", post(model)),
        )
        .await
        .unwrap()
    });
    let config = root.path().join("config.toml");
    let state = root.path().join("state");
    std::fs::write(
        &config,
        format!(
            r#"
[llm]
base_url = {url:?}
model = "fixture"
api_key = "fixture-key"
[session]
directory = {:?}
[skills]
enabled = false
[sandbox]
mode = "read-only"
allow_commands = false
"#,
            state.join("sessions").to_string_lossy()
        ),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_kyler"))
        .arg("--config")
        .arg(&config)
        .arg("acp")
        .env("HOME", root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    async fn send(input: &mut tokio::process::ChildStdin, id: u32, method: &str, params: Value) {
        input
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        input.flush().await.unwrap();
    }
    async fn response(
        lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
        id: u32,
    ) -> Value {
        tokio::time::timeout(Duration::from_secs(60), async {
            let mut notifications = Vec::new();
            loop {
                let line = lines.next_line().await.unwrap().expect("ACP stdout closed");
                let mut value: Value =
                    serde_json::from_str(&line).expect("stdout must contain only JSON-RPC");
                if value["method"] == "session/update" {
                    notifications.push(value.clone());
                }
                if value["id"] == id {
                    value["captured_updates"] = json!(notifications);
                    return value;
                }
            }
        })
        .await
        .unwrap()
    }
    send(
        &mut input,
        1,
        "initialize",
        json!({"protocolVersion":1,"clientCapabilities":{}}),
    )
    .await;
    assert_eq!(
        response(&mut lines, 1).await["result"]["protocolVersion"],
        1
    );
    send(
        &mut input,
        2,
        "session/new",
        json!({"cwd":project,"mcpServers":[]}),
    )
    .await;
    let session = response(&mut lines, 2).await;
    assert!(session.get("error").is_none(), "{session}");
    let id = session["result"]["sessionId"].as_str().unwrap();
    send(
        &mut input,
        3,
        "session/prompt",
        json!({"sessionId":id,"prompt":[{"type":"text","text":"hello"}]}),
    )
    .await;
    let result = response(&mut lines, 3).await;
    assert_eq!(result["result"]["stopReason"], "end_turn", "{result}");
    assert!(
        result["captured_updates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|notification| notification["params"]["update"]["sessionUpdate"] == "plan"),
        "{result}"
    );
    assert!(
        state
            .join("acp/sessions")
            .join(id)
            .join("rollout.jsonl")
            .exists()
    );
    assert!(!state.join("tui/sessions").exists());
    assert!(!state.join("sessions").exists());
    drop(input);
    assert!(
        tokio::time::timeout(Duration::from_secs(15), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    upstream.abort();
}

#[tokio::test]
async fn incompatible_cli_flags_never_write_non_protocol_stdout() {
    for flags in [
        vec!["--json", "acp"],
        vec!["--server", "http://127.0.0.1:1", "acp"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_kyler"))
            .args(flags)
            .output()
            .await
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}
