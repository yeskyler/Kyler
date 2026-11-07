use std::{
    convert::Infallible,
    future::IntoFuture,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, Sse},
    },
    routing::post,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use kyler_config::{AppConfig, McpServerSettings, McpTransport, SecretString};
use kyler_protocol::api::{
    event::{EventPayload, ServerEvent},
    mcp::{McpServerInfo, McpStatus},
    run::{RunAccepted, RunInfo, RunStatus},
    session::SessionSummary,
};
use kyler_server::Server;

async fn remote(Json(request): Json<Value>) -> axum::response::Response {
    if request.get("id").is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    let result = match request["method"].as_str().unwrap() {
        "initialize" => {
            json!({"protocolVersion":request["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"remote","version":"1"}})
        }
        "tools/list" => {
            json!({"tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object"}}]})
        }
        "tools/call" => {
            json!({"content":[{"type":"text","text":"remote result"}],"structuredContent":{"value":42}})
        }
        _ => json!({}),
    };
    Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result})).into_response()
}
fn chunk(delta: Value, reason: Value) -> String {
    json!({"id":"test","object":"chat.completion.chunk","created":0,"model":"test","choices":[{"index":0,"delta":delta,"finish_reason":reason}]}).to_string()
}
async fn model(
    State(requests): State<Arc<Mutex<Vec<Value>>>>,
    Json(request): Json<Value>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    requests.lock().unwrap().push(request.clone());
    let has_tool = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["function"]["name"] == "mcp__remote__echo");
    let has_result = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "tool");
    let text = if has_tool && !has_result {
        chunk(
            json!({"tool_calls":[{"index":0,"id":"mcp-call","type":"function","function":{"name":"mcp__remote__echo","arguments":"{}"}}]}),
            json!("tool_calls"),
        )
    } else {
        chunk(
            json!({"content":if has_tool {"MCP completed"}else{"MCP disconnected"}}),
            json!("stop"),
        )
    };
    Sse::new(futures::stream::iter(vec![
        Ok(Event::default().data(text)),
        Ok(Event::default().data("[DONE]")),
    ]))
}
async fn servers(http: &reqwest::Client, base: &str, session: &str) -> Vec<McpServerInfo> {
    http.get(format!("{base}/v1/sessions/{session}/mcp"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}
async fn wait(http: &reqwest::Client, base: &str, session: &str, status: McpStatus) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if servers(http, base, session).await[0].status == status {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
async fn run(http: &reqwest::Client, base: &str, session: &str) -> (RunInfo, Vec<ServerEvent>) {
    http.put(format!("{base}/v1/sessions/{session}/permissions"))
        .json(&json!({"approval":{"policy":"never","reviewer":"user"}}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let accepted: RunAccepted = http
        .post(format!("{base}/v1/sessions/{session}/runs"))
        .json(&json!({"input":"Use available tools"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let text = http
        .get(format!("{base}/v1/runs/{}/events", accepted.run_id))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let events = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str::<ServerEvent>(data).unwrap())
        .collect();
    let info = http
        .get(format!("{base}/v1/runs/{}", accepted.run_id))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (info, events)
}
#[tokio::test]
async fn mcp_management_and_dynamic_agent_tools_work_end_to_end_per_session() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("http://{}", listener.local_addr().unwrap());
    let fixture = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/mcp", post(remote))
                .route("/v1/chat/completions", post(model))
                .with_state(requests.clone()),
        )
        .into_future(),
    );
    let root = tempfile::tempdir().unwrap();
    let mut config = AppConfig::default();
    config.llm.base_url = format!("{upstream}/v1");
    config.llm.model = "test".into();
    config.llm.api_key = SecretString::new("fixture");
    config.session.enabled = true;
    config.session.directory = root.path().join("sessions");
    config.mcp.servers.insert(
        "remote".into(),
        McpServerSettings {
            transport: McpTransport::StreamableHttp,
            url: format!("{upstream}/mcp"),
            activity: "search".into(),
            ..Default::default()
        },
    );
    let server = Server::from_config(&config).await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.serve(listener, async {
        let _ = stopped.await;
    }));
    let http = reqwest::Client::new();
    let first: SessionSummary = http
        .post(format!("{base}/v1/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second: SessionSummary = http
        .post(format!("{base}/v1/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    wait(&http, &base, &first.session_id, McpStatus::Connected).await;
    wait(&http, &base, &second.session_id, McpStatus::Connected).await;
    let (result, events) = run(&http, &base, &first.session_id).await;
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(result.output.as_deref(), Some("MCP completed"));
    assert!(events.iter().any(|e|matches!(&e.payload,EventPayload::ToolStarted{source:kyler_protocol::activity::ToolSource::Mcp{server_id,..},activity:kyler_protocol::activity::ActivityKind::Search,..} if server_id=="remote")));
    assert!(
        requests.lock().unwrap().iter().any(|r| r["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "tool"
                && m["content"].as_str().unwrap().contains("structuredContent")))
    );
    assert_eq!(
        http.post(format!(
            "{base}/v1/sessions/{}/mcp/remote/disconnect",
            first.session_id
        ))
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::ACCEPTED
    );
    wait(&http, &base, &first.session_id, McpStatus::Disconnected).await;
    assert_eq!(
        servers(&http, &base, &second.session_id).await[0].status,
        McpStatus::Connected
    );
    let (result, _) = run(&http, &base, &first.session_id).await;
    assert_eq!(result.output.as_deref(), Some("MCP disconnected"));
    assert_eq!(
        http.post(format!(
            "{base}/v1/sessions/{}/mcp/unknown/connect",
            first.session_id
        ))
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        http.post(format!(
            "{base}/v1/sessions/{}/mcp/remote/invalid",
            first.session_id
        ))
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::BAD_REQUEST
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
    fixture.abort();
}
