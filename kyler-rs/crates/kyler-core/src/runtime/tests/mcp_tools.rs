use super::*;
use crate::{
    runtime::mcp_tools::McpRetrieval,
    tools::{ToolExecutionMode, ToolSource},
};

use std::sync::atomic::{AtomicUsize, Ordering};

struct Remote {
    name: String,
    remote: String,
    description: String,
    schema: Value,
    calls: Arc<AtomicUsize>,
    version: String,
}
#[async_trait::async_trait]
impl Tool for Remote {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn parameters(&self) -> Value {
        self.schema.clone()
    }
    fn source(&self) -> ToolSource {
        ToolSource::Mcp {
            server_id: "fixture".into(),
            remote_name: self.remote.clone(),
        }
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Mcp
    }
    fn approval_identity(&self) -> String {
        format!("{}:{}", self.name, self.version)
    }
    async fn execute(&self, _: &str, _: &mut ToolContext<'_>) -> Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok("remote result".into())
    }
}
fn remote(name: &str, description: &str, calls: &Arc<AtomicUsize>) -> Arc<dyn Tool> {
    Arc::new(Remote {
        name: format!("mcp__fixture__{name}"),
        remote: name.into(),
        description: description.into(),
        schema: json!({"type":"object","properties":{}}),
        calls: calls.clone(),
        version: "one".into(),
    })
}
fn tools(calls: &Arc<AtomicUsize>) -> Arc<ToolBox> {
    let mut tools = ToolBox::new();
    for (name, description) in [
        ("searchEmails", "Find email messages"),
        ("createIssue", "Create an issue ticket"),
    ] {
        let tool = remote(name, description, calls);
        tools.insert(tool.name().into(), tool);
    }
    for i in 0..8 {
        let tool = remote(&format!("unrelated{i}"), "Miscellaneous fixture", calls);
        tools.insert(tool.name().into(), tool);
    }
    Arc::new(tools)
}
fn call(id: &str, name: &str, args: Value) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}})
}
fn advertised(request: &Value) -> Vec<&str> {
    request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn auto_retrieval_exposes_relevant_tools_before_first_request_without_search_round_trip() {
    let (url,requests,server)=server(vec![json!({"role":"assistant","tool_calls":[call("mail","mcp__fixture__searchEmails",json!({}))]}),json!({"role":"assistant","content":"done"})]).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = Agent::new(
        url,
        "fixture".into(),
        "fixture",
        None::<String>,
        tools(&calls),
    );
    agent
        .run_in_session(&mut Session::new(), "查找邮件")
        .await
        .unwrap();
    server.await.unwrap();
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert!(advertised(&requests[0]).contains(&"mcp__fixture__searchEmails"));
    assert!(!advertised(&requests[0]).contains(&"mcp__fixture__unrelated0"));
    assert_eq!(advertised(&requests[0]), advertised(&requests[1]));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn searched_tools_are_only_callable_in_next_request_and_results_omit_full_schemas() {
    let (url,requests,server)=server(vec![
        json!({"role":"assistant","tool_calls":[call("search","search_tools",json!({"queries":["createIssue"]})),call("too_early","mcp__fixture__createIssue",json!({}))]}),
        json!({"role":"assistant","tool_calls":[call("issue","mcp__fixture__createIssue",json!({}))]}),
        json!({"role":"assistant","content":"done"}),
    ]).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = Agent::new(
        url,
        "fixture".into(),
        "fixture",
        None::<String>,
        tools(&calls),
    );
    let mut session = Session::new();
    agent
        .run_in_session(&mut session, "unrelatedzz")
        .await
        .unwrap();
    server.await.unwrap();
    let requests = requests.lock().await;
    assert!(!advertised(&requests[0]).contains(&"mcp__fixture__createIssue"));
    assert!(advertised(&requests[1]).contains(&"mcp__fixture__createIssue"));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "same-batch hidden calls must not execute"
    );
    let search = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["tool_call_id"] == "search")
        .unwrap();
    let result: Value = serde_json::from_str(search["content"].as_str().unwrap()).unwrap();
    assert_eq!(result["matches"][0]["activated"], true);
    assert!(result["matches"][0]["tool"].get("parameters").is_none());
    assert!(session.events.iter().flat_map(|event|&event.content).any(|item|matches!(item,ContentItem::ToolResult {tool_call_id,status:ToolResultStatus::Error,..} if tool_call_id=="too_early")));
}
#[tokio::test]
async fn small_catalog_is_direct_and_large_schema_is_not_silently_truncated() {
    let calls = Arc::new(AtomicUsize::new(0));
    let small = remote("echo", "Echo", &calls);
    let huge: Arc<dyn Tool> = Arc::new(Remote {
        name: "mcp__fixture__huge".into(),
        remote: "huge".into(),
        description: "Huge tool".into(),
        schema: json!({"type":"object","properties":{"text":{"type":"string","enum":(0..5000).map(|i|format!("value{i}")).collect::<Vec<_>>()}}}),
        calls,
        version: "one".into(),
    });
    let mut agent = Agent::new(
        "http://127.0.0.1:1".into(),
        "fixture".into(),
        "fixture",
        None::<String>,
        Arc::new([(small.name().into(), small), (huge.name().into(), huge)].into()),
    );
    let mut session = Session::new();
    session.begin_execution("huge").unwrap();
    let mut retrieval = McpRetrieval::default();
    agent
        .select_mcp_tools(&session, &mut retrieval, "0")
        .await
        .unwrap();
    assert!(!agent.toolbox.contains_key("mcp__fixture__huge"));
    let call: ChatCompletionMessageToolCalls =
        serde_json::from_value(call("search", "search_tools", json!({"queries":["huge"]})))
            .unwrap();
    agent
        .record_tool_calls(&mut session, &[call.clone()])
        .unwrap();
    agent
        .execute_tool_calls(&mut session, &[call], None, None)
        .await
        .unwrap();
    let output = session
        .events
        .iter()
        .flat_map(|e| &e.content)
        .find_map(|item| {
            if let ContentItem::ToolResult {
                tool_call_id,
                content,
                ..
            } = item
            {
                (tool_call_id == "search").then_some(content)
            } else {
                None
            }
        })
        .unwrap();
    let output: Value = serde_json::from_str(output).unwrap();
    assert_eq!(output["matches"][0]["activated"], false);
    let small = remote("echo", "Echo", &Arc::new(AtomicUsize::new(0)));
    let mut agent = Agent::new(
        "http://127.0.0.1:1".into(),
        "fixture".into(),
        "fixture",
        None::<String>,
        Arc::new([(small.name().into(), small)].into()),
    );
    agent
        .select_mcp_tools(&session, &mut McpRetrieval::default(), "0")
        .await
        .unwrap();
    assert!(agent.toolbox.contains_key("mcp__fixture__echo"));
}
#[tokio::test]
async fn task_activation_is_isolated_and_catalog_removal_revokes_tools() {
    let calls = Arc::new(AtomicUsize::new(0));
    let all = tools(&calls);
    let base = Agent::new(
        "http://127.0.0.1:1".into(),
        "fixture".into(),
        "fixture",
        None::<String>,
        all.clone(),
    );
    let mut mail = Session::new();
    mail.begin_execution("查找邮件").unwrap();
    let mut issue = Session::new();
    issue.begin_execution("创建工单").unwrap();
    let mut first = base.clone();
    let mut state = McpRetrieval::default();
    first
        .select_mcp_tools(&mail, &mut state, "0")
        .await
        .unwrap();
    assert!(first.toolbox.contains_key("mcp__fixture__searchEmails"));
    let mut second = base.clone();
    second
        .select_mcp_tools(&issue, &mut McpRetrieval::default(), "0")
        .await
        .unwrap();
    assert!(second.toolbox.contains_key("mcp__fixture__createIssue"));
    assert!(!second.toolbox.contains_key("mcp__fixture__searchEmails"));
    let mut removed = base.clone();
    let mut remaining = all.as_ref().clone();
    remaining.remove("mcp__fixture__searchEmails");
    removed.toolbox = Arc::new(remaining);
    removed
        .select_mcp_tools(&mail, &mut state, "1")
        .await
        .unwrap();
    assert!(!removed.toolbox.contains_key("mcp__fixture__searchEmails"));
}

#[tokio::test]
async fn a_connecting_server_does_not_delay_the_first_model_request() {
    use axum::{
        Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post,
    };
    use kyler_config::{McpServerSettings, McpSettings, McpTransport};
    struct Gate {
        started: tokio::sync::Notify,
        release: tokio::sync::Semaphore,
    }
    async fn handler(
        State(gate): State<Arc<Gate>>,
        Json(request): Json<Value>,
    ) -> axum::response::Response {
        let Some(id) = request.get("id") else {
            return StatusCode::ACCEPTED.into_response();
        };
        let result = match request["method"].as_str().unwrap() {
            "initialize" => {
                gate.started.notify_one();
                gate.release.acquire().await.unwrap().forget();
                json!({"protocolVersion":request["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"slow","version":"1"}})
            }
            _ => json!({"tools":[]}),
        };
        Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
    }
    let gate = Arc::new(Gate {
        started: tokio::sync::Notify::new(),
        release: tokio::sync::Semaphore::new(0),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let remote_gate = gate.clone();
    let remote = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/mcp", post(handler))
                .with_state(remote_gate),
        )
        .await
        .unwrap()
    });
    let manager = Arc::new(
        McpManager::new(McpSettings {
            servers: [(
                "slow".into(),
                McpServerSettings {
                    transport: McpTransport::StreamableHttp,
                    url,
                    connect_timeout_seconds: 60,
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        })
        .unwrap(),
    );
    let (url, requests, model) = server(vec![
        json!({"role":"assistant","content":"local work completed"}),
    ])
    .await;
    #[derive(Default)]
    struct Observations(std::sync::Mutex<Vec<crate::runtime::observation::RuntimeObservation>>);
    impl crate::runtime::observation::RuntimeObserver for Observations {
        fn observe(&self, event: crate::runtime::observation::RuntimeObservation) {
            self.0.lock().unwrap().push(event);
        }
    }
    let observations = Arc::new(Observations::default());
    let agent = Agent::new(
        url,
        "fixture".into(),
        "fixture",
        None::<String>,
        Arc::new(ToolBox::new()),
    )
    .with_mcp_manager(manager.clone())
    .with_observer(observations.clone());
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        agent.run_in_session(&mut Session::new(), "answer a local question"),
    )
    .await;
    assert!(
        result.is_ok(),
        "Model request blocked; observations: {:?}; received requests: {}",
        observations.0.lock().unwrap(),
        requests.lock().await.len()
    );
    let result = result.unwrap().unwrap();
    assert_eq!(result.output, "local work completed");
    model.await.unwrap();
    assert!(advertised(&requests.lock().await[0]).contains(&"search_tools"));
    assert_eq!(
        manager
            .list(&result.session.session_id, None)
            .await
            .unwrap()[0]
            .status,
        kyler_protocol::api::mcp::McpStatus::Connecting
    );
    gate.release.add_permits(1);
    manager.shutdown().await;
    remote.abort();
}

#[tokio::test]
async fn readonly_policy_hides_discovery_and_remote_tools() {
    let calls = Arc::new(AtomicUsize::new(0));
    let directory = tempfile::tempdir().unwrap();
    let environment = Arc::new(
        ExecutionEnvironment::mxc(
            directory.path(),
            directory.path(),
            crate::sandbox::SandboxPolicy::read_only("fixture", directory.path()),
            std::env::current_exe().unwrap(),
        )
        .unwrap(),
    );
    let mut agent = Agent::new(
        "http://127.0.0.1:1".into(),
        "fixture".into(),
        "fixture",
        None::<String>,
        tools(&calls),
    )
    .with_environment(environment);
    let mut session = Session::new();
    session.begin_execution("查找邮件").unwrap();
    agent
        .select_mcp_tools(&session, &mut McpRetrieval::default(), "one")
        .await
        .unwrap();
    assert!(!agent.toolbox.contains_key("search_tools"));
    assert!(
        !agent
            .toolbox
            .values()
            .any(|tool| matches!(tool.source(), ToolSource::Mcp { .. }))
    );
}
