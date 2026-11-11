use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

use kyler_core::{
    Agent, AgentStreamEvent, ExecutionStatus, Session, SessionStore, Tool, ToolBox, ToolContext,
};

fn chunk(delta: Value, reason: Value) -> Value {
    json!({"id":"test", "object":"chat.completion.chunk", "created":0,"model":"test",
        "choices":[{"index":0,"delta":delta,"finish_reason":reason}]})
}

async fn fixture(responses: Vec<Vec<Value>>) -> (String, JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for chunks in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = Vec::new();
            let mut buffer = [0; 4096];
            let (start, length) = loop {
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                data.extend_from_slice(&buffer[..n]);
                if let Some(end) = data.windows(4).position(|x| x == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&data[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break (end + 4, length);
                }
            };
            while data.len() < start + length {
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                data.extend_from_slice(&buffer[..n]);
            }
            requests.push(serde_json::from_slice(&data[start..start + length]).unwrap());
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").await.unwrap();
            for value in chunks {
                if value.get("_stall").is_some() {
                    // Keep the response open until the client cancels/drops it.
                    let mut byte = [0];
                    let _ = socket.read(&mut byte).await;
                    break;
                }
                // Real, separate network writes exercise incremental SSE decoding.
                if socket
                    .write_all(format!("data: {value}\n\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
            let _ = socket.write_all(b"data: [DONE]\n\n").await;
        }
        requests
    });
    (format!("http://{address}/v1"), task)
}

fn agent(url: String, toolbox: ToolBox) -> Agent {
    Agent::new(
        url,
        "fixture".into(),
        "test",
        Some("help"),
        Arc::new(toolbox),
    )
}
fn answer(text: &str) -> Vec<Value> {
    vec![
        chunk(json!({"content":text}), Value::Null),
        chunk(json!({}), json!("stop")),
        json!({"id":"test","object":"chat.completion.chunk","created":0,"model":"test","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}),
    ]
}

fn usage(prompt: u32, completion: u32) -> Value {
    json!({"id":"test","object":"chat.completion.chunk","created":0,"model":"test","choices":[],
        "usage":{"prompt_tokens":prompt,"completion_tokens":completion,"total_tokens":prompt+completion}})
}

#[tokio::test]
async fn cumulative_stream_usage_is_counted_once_per_request_and_survives_reload() {
    let response = vec![
        usage(1000, 1),
        chunk(json!({"content":"done"}), Value::Null),
        usage(1000, 2),
        chunk(json!({}), json!("stop")),
        usage(1000, 2),
        usage(900, 1),
        usage(0, 0),
    ];
    let (url, server) = fixture(vec![response, answer("again")]).await;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::new(directory.path()).await.unwrap());
    let agent = agent(url, ToolBox::new()).with_session_store(store.clone());
    let first = agent
        .run_stream("first")
        .await
        .unwrap()
        .finish()
        .await
        .unwrap();
    assert_eq!(first.context.usage.prompt_tokens, 1000);
    assert_eq!(first.context.usage.completion_tokens, 2);
    assert_eq!(first.context.usage.total_tokens, 1002);
    let restored = store.load(&first.session.session_id).await.unwrap();
    assert_eq!(restored.usage.total_tokens, 1002);
    assert_eq!(
        restored
            .memory
            .last_request
            .as_ref()
            .unwrap()
            .provider_input_tokens,
        Some(1000)
    );
    let second = agent
        .run_stream_in_session(restored, "second")
        .await
        .unwrap()
        .finish()
        .await
        .unwrap();
    assert_eq!(second.context.usage.total_tokens, 12);
    assert_eq!(second.session.usage.prompt_tokens, 1010);
    assert_eq!(second.session.usage.completion_tokens, 4);
    assert_eq!(second.session.usage.total_tokens, 1014);
    assert_eq!(
        second
            .session
            .memory
            .last_request
            .unwrap()
            .provider_input_tokens,
        Some(10)
    );
    assert_eq!(server.await.unwrap().len(), 2);
}

#[tokio::test]
async fn cancelling_a_stream_preserves_known_usage_without_double_counting() {
    let (url, server) = fixture(vec![vec![
        usage(1000, 1),
        usage(1000, 2),
        usage(1000, 2),
        chunk(json!({"content":"first"}), Value::Null),
        json!({"_stall":true}),
    ]])
    .await;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::new(directory.path()).await.unwrap());
    let mut stream = agent(url, ToolBox::new())
        .with_session_store(store.clone())
        .run_stream("wait")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.next().await {
            if matches!(event, AgentStreamEvent::TextDelta { .. }) {
                return;
            }
        }
        panic!("stream ended before text");
    })
    .await
    .unwrap();
    stream.cancel();
    let outcome = stream.finish_with_session().await.unwrap();
    assert!(outcome.result.is_err());
    let restored = store.load(&outcome.session.session_id).await.unwrap();
    assert_eq!(restored.usage.total_tokens, 1002);
    assert_eq!(
        restored.memory.last_request.unwrap().provider_input_tokens,
        Some(1000)
    );
    assert_eq!(
        restored.execution.unwrap().status,
        ExecutionStatus::Cancelled
    );
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

struct Echo(Arc<AtomicUsize>);
#[async_trait]
impl Tool for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "echo"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"text":{"type":"string"}}})
    }
    async fn execute(&self, args: &str, _: &mut ToolContext<'_>) -> anyhow::Result<String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::from_str::<Value>(args)?["text"]
            .as_str()
            .unwrap()
            .into())
    }
}

#[tokio::test]
async fn failed_stream_preserves_reported_usage_in_its_saved_session() {
    let (url, server) = fixture(vec![vec![
        usage(1000, 2),
        usage(1000, 2),
        chunk(json!({"content":"interrupted"}), Value::Null),
    ]])
    .await;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::new(directory.path()).await.unwrap());
    let outcome = agent(url, ToolBox::new())
        .with_session_store(store.clone())
        .run_stream("test")
        .await
        .unwrap()
        .finish_with_session()
        .await
        .unwrap();
    assert!(outcome.result.is_err());
    let saved = store.load(&outcome.session.session_id).await.unwrap();
    assert_eq!(saved.usage.total_tokens, 1002);
    assert_eq!(saved.execution.unwrap().status, ExecutionStatus::Failed);
    server.await.unwrap();
}

#[tokio::test]
async fn fragmented_tool_arguments_events_usage_and_followup() {
    let calls = vec![
        chunk(
            json!({"tool_calls":[{"index":0,"id":"call1","type":"function","function":{"name":"ec","arguments":"{\"te"}}]}),
            Value::Null,
        ),
        chunk(
            json!({"tool_calls":[{"index":0,"function":{"name":"ho","arguments":"xt\":\"\u{4f60}\u{597d}\"}"}}]}),
            Value::Null,
        ),
        chunk(json!({}), json!("tool_calls")),
    ];
    let (url, server) = fixture(vec![calls, answer("Done"), answer("\u{7ee7}\u{7eed}")]).await;
    let count = Arc::new(AtomicUsize::new(0));
    let tools = ToolBox::from([(
        "echo".into(),
        Arc::new(Echo(count.clone())) as Arc<dyn Tool>,
    )]);
    let agent = agent(url, tools);
    let mut stream = agent.run_stream("\u{5f00}\u{59cb}").await.unwrap();
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    let result = stream.finish().await.unwrap();
    assert_eq!(result.output, "Done");
    assert_eq!(result.context.usage.total_tokens, 12);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(matches!(
        events.first(),
        Some(AgentStreamEvent::Started { .. })
    ));
    let start=events.iter().position(|e|matches!(e,AgentStreamEvent::ToolStarted{arguments,..} if arguments==&json!({"text":"\u{4f60}\u{597d}"}))).unwrap();
    let preparing = events.iter().position(|e| matches!(e, AgentStreamEvent::ToolCallDelta { call_id, name } if call_id == "call1" && name == "ec")).unwrap();
    let named = events.iter().position(|e| matches!(e, AgentStreamEvent::ToolCallDelta { call_id, name } if call_id == "call1" && name == "echo")).unwrap();
    assert!(preparing < named && named < start);
    let end = events
        .iter()
        .position(
            |e| matches!(e,AgentStreamEvent::ToolFinished{output,..} if output=="\u{4f60}\u{597d}"),
        )
        .unwrap();
    assert!(start < end);
    assert!(matches!(
        events.last(),
        Some(AgentStreamEvent::Completed { .. })
    ));
    let session = agent
        .run_stream_in_session(result.session, "\u{7ee7}\u{7eed}")
        .await
        .unwrap()
        .finish()
        .await
        .unwrap();
    assert_eq!(session.output, "\u{7ee7}\u{7eed}");
    let requests = server.await.unwrap();
    assert_eq!(requests[0]["stream"], true);
    assert_eq!(requests[0]["stream_options"]["include_usage"], true);
    assert!(
        requests[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "tool" && m["content"] == "\u{4f60}\u{597d}")
    );
}

#[tokio::test]
async fn finish_drains_more_than_channel_capacity() {
    let mut chunks = (0..200)
        .map(|_| chunk(json!({"content":"x"}), Value::Null))
        .collect::<Vec<_>>();
    chunks.push(chunk(json!({}), json!("stop")));
    let (url, server) = fixture(vec![chunks]).await;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        agent(url, ToolBox::new())
            .run_stream("test")
            .await
            .unwrap()
            .finish(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.output.len(), 200);
    assert_eq!(result.session.usage.total_tokens, 0);
    assert!(
        result
            .session
            .memory
            .last_request
            .unwrap()
            .provider_input_tokens
            .is_none()
    );
    server.await.unwrap();
}

#[tokio::test]
async fn truncated_or_invalid_tool_stream_never_executes() {
    for chunks in [
        vec![chunk(
            json!({"tool_calls":[{"index":0,"id":"c","function":{"name":"echo","arguments":"{}"}}]}),
            Value::Null,
        )],
        vec![chunk(
            json!({"tool_calls":[{"index":0,"id":"c","function":{"name":"echo","arguments":"{broken"}}]}),
            json!("tool_calls"),
        )],
    ] {
        let (url, server) = fixture(vec![chunks]).await;
        let count = Arc::new(AtomicUsize::new(0));
        let tools = ToolBox::from([(
            "echo".into(),
            Arc::new(Echo(count.clone())) as Arc<dyn Tool>,
        )]);
        let outcome = agent(url, tools)
            .run_stream("test")
            .await
            .unwrap()
            .finish_with_session()
            .await
            .unwrap();
        assert!(outcome.result.is_err());
        assert_eq!(
            outcome.session.execution.unwrap().status,
            ExecutionStatus::Failed
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
        server.await.unwrap();
    }
}

struct Slow;
#[async_trait]
impl Tool for Slow {
    fn name(&self) -> &str {
        "slow"
    }
    fn description(&self) -> &str {
        "wait"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object"})
    }
    async fn execute(&self, _: &str, _: &mut ToolContext<'_>) -> anyhow::Result<String> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn cancellation_saves_session_and_repairs_unfinished_tool() {
    let (url,server)=fixture(vec![vec![chunk(json!({"tool_calls":[{"index":0,"id":"slow1","function":{"name":"slow","arguments":"{}"}}]}),json!("tool_calls"))]]).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::new(dir.path()).await.unwrap());
    let tools = ToolBox::from([("slow".into(), Arc::new(Slow) as Arc<dyn Tool>)]);
    let session = Session::new();
    let id = session.session_id.clone();
    let mut stream = agent(url, tools)
        .with_session_store(store.clone())
        .run_stream_in_session(session, "wait")
        .await
        .unwrap();
    loop {
        if matches!(
            stream.next().await,
            Some(AgentStreamEvent::ToolStarted { .. })
        ) {
            break;
        }
    }
    stream.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(5), stream.finish_with_session())
        .await
        .unwrap()
        .unwrap();
    assert!(outcome.result.is_err());
    assert_eq!(
        outcome.session.execution.as_ref().unwrap().status,
        ExecutionStatus::Cancelled
    );
    let saved = store.load(&id).await.unwrap();
    assert_eq!(saved.execution.unwrap().status, ExecutionStatus::Cancelled);
    assert!(outcome.session.events.iter().flat_map(|e|&e.content).any(|c|matches!(c,kyler_core::context::event::ContentItem::ToolResult{tool_call_id,..} if tool_call_id=="slow1")));
    server.await.unwrap();
}

#[tokio::test]
async fn text_arrives_before_model_finishes_and_cancel_closes_connection() {
    let (url, server) = fixture(vec![vec![
        chunk(json!({"content":"first"}), Value::Null),
        json!({"_stall":true}),
    ]])
    .await;
    let mut stream = agent(url, ToolBox::new()).run_stream("wait").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stream.next().await {
                Some(AgentStreamEvent::TextDelta { text, .. }) => {
                    assert_eq!(text, "first");
                    break;
                }
                Some(_) => {}
                None => panic!("stream closed before text"),
            }
        }
    })
    .await
    .unwrap();
    assert!(!server.is_finished(), "model response must still be open");
    stream.cancel();
    let mut cancelled = false;
    while let Some(event) = stream.next().await {
        cancelled |= matches!(event, AgentStreamEvent::Cancelled);
    }
    assert!(cancelled);
    let outcome = stream.finish_with_session().await.unwrap();
    assert_eq!(
        outcome.session.execution.unwrap().status,
        ExecutionStatus::Cancelled
    );
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn dropping_handle_cancels_and_persists_without_a_consumer() {
    let (url, server) = fixture(vec![vec![
        chunk(json!({"content":"first"}), Value::Null),
        json!({"_stall":true}),
    ]])
    .await;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::new(directory.path()).await.unwrap());
    let session = Session::new();
    let id = session.session_id.clone();
    let mut stream = agent(url, ToolBox::new())
        .with_session_store(store.clone())
        .run_stream_in_session(session, "wait")
        .await
        .unwrap();
    loop {
        match stream.next().await {
            Some(AgentStreamEvent::TextDelta { .. }) => break,
            Some(_) => {}
            None => panic!("stream ended"),
        }
    }
    drop(stream);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(session) = store.load(&id).await {
                if session
                    .execution
                    .as_ref()
                    .is_some_and(|e| e.status == ExecutionStatus::Cancelled)
                {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn provider_reasoning_streams_and_persists_without_becoming_answer_or_request_mode() {
    use kyler_core::context::ContentItem;
    let response = vec![
        chunk(json!({"reasoning_content":"Inspect "}), Value::Null),
        chunk(json!({"reasoning_content":"the project."}), Value::Null),
        chunk(json!({"content":"Done."}), Value::Null),
        chunk(json!({}), json!("stop")),
    ];
    let (url, server) = fixture(vec![response]).await;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::new(directory.path()).await.unwrap());
    let mut stream = agent(url, ToolBox::new())
        .with_session_store(store.clone())
        .run_stream("Inspect")
        .await
        .unwrap();
    let mut reasoning = String::new();
    let mut answer = String::new();
    while let Some(event) = stream.next().await {
        match event {
            AgentStreamEvent::ReasoningDelta { text, .. } => reasoning.push_str(&text),
            AgentStreamEvent::TextDelta { text, .. } => answer.push_str(&text),
            _ => {}
        }
    }
    let result = stream.finish().await.unwrap();
    assert_eq!(reasoning, "Inspect the project.");
    assert_eq!(answer, "Done.");
    assert_eq!(result.output, "Done.");
    let restored = store.load(&result.session.session_id).await.unwrap();
    assert!(
        restored
            .events
            .iter()
            .flat_map(|event| &event.content)
            .any(|item| matches!(item,ContentItem::Reasoning {text} if text==&reasoning))
    );
    assert!(restored.events.iter().all(|event| {
        event
            .model_view()
            .content
            .iter()
            .all(|item| !matches!(item, ContentItem::Reasoning { .. }))
    }));
    let requests = server.await.unwrap();
    assert_eq!(requests[0]["stream"], true);
    for key in ["reasoning_effort", "thinking", "enable_thinking"] {
        assert!(requests[0].get(key).is_none());
    }
}

#[tokio::test]
async fn tool_batch_is_queued_before_first_tool_finishes_and_execution_stays_serial() {
    struct Gate {
        calls: Arc<AtomicUsize>,
        release: Arc<tokio::sync::Notify>,
    }
    #[async_trait]
    impl Tool for Gate {
        fn name(&self) -> &str {
            "gate"
        }
        fn description(&self) -> &str {
            "Local batch fixture"
        }
        fn parameters(&self) -> Value {
            json!({"type":"object","properties":{}})
        }
        async fn execute(&self, _: &str, _: &mut ToolContext<'_>) -> anyhow::Result<String> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.release.notified().await;
            }
            Ok("done".into())
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let tools = ToolBox::from([(
        "gate".into(),
        Arc::new(Gate {
            calls: calls.clone(),
            release: release.clone(),
        }) as Arc<dyn Tool>,
    )]);
    let (url, server) = fixture(vec![
        vec![chunk(
            json!({"tool_calls":[
                {"index":0,"id":"a","type":"function","function":{"name":"gate","arguments":"{}"}},
                {"index":1,"id":"b","type":"function","function":{"name":"gate","arguments":"{}"}}
            ]}),
            json!("tool_calls"),
        )],
        answer("Done"),
    ])
    .await;
    let mut stream = agent(url, tools).run_stream("batch").await.unwrap();
    let mut order = Vec::new();
    tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(event) = stream.next().await {
            match event {
                AgentStreamEvent::ToolQueued { call_id, .. } => {
                    order.push(format!("queued:{call_id}"))
                }
                AgentStreamEvent::ToolStarted { call_id, .. } => {
                    order.push(format!("started:{call_id}"));
                    if call_id == "a" {
                        assert_eq!(order, ["queued:a", "queued:b", "started:a"]);
                        assert!(calls.load(Ordering::SeqCst) <= 1);
                        release.notify_one();
                    }
                }
                AgentStreamEvent::ToolFinished { call_id, .. } => {
                    order.push(format!("finished:{call_id}"))
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    stream.finish().await.unwrap();
    assert_eq!(
        order,
        [
            "queued:a",
            "queued:b",
            "started:a",
            "finished:a",
            "started:b",
            "finished:b"
        ]
    );
    server.await.unwrap();
}

#[tokio::test]
async fn tool_preparation_is_visible_before_arguments_finish() {
    let (url, server) = fixture(vec![vec![
        chunk(json!({"tool_calls":[{"index":0,"id":"partial","function":{"name":"echo","arguments":"{"}}]}), Value::Null),
        json!({"_stall": true}),
    ]]).await;
    let count = Arc::new(AtomicUsize::new(0));
    let tools = ToolBox::from([(
        "echo".into(),
        Arc::new(Echo(count.clone())) as Arc<dyn Tool>,
    )]);
    let mut stream = agent(url, tools).run_stream("prepare").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stream.next().await.unwrap() {
                AgentStreamEvent::ToolCallDelta { call_id, name } => {
                    assert_eq!(call_id, "partial");
                    assert_eq!(name, "echo");
                    break;
                }
                AgentStreamEvent::ToolStarted { .. } => {
                    panic!("Incomplete arguments must never execute")
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 0);
    stream.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(5), stream.finish_with_session())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        outcome.session.execution.unwrap().status,
        ExecutionStatus::Cancelled
    );
    server.await.unwrap();
}
