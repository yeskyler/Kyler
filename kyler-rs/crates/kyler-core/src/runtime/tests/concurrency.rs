use super::*;
use crate::tools::{ParallelToolContext, ToolConcurrency, ToolExecutionMode};

use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
};

use tokio::sync::{Semaphore, mpsc};

struct Probe {
    name: &'static str,
    lane: ToolConcurrency,
    active: Arc<AtomicUsize>,
    started: mpsc::UnboundedSender<String>,
    gates: Arc<HashMap<String, Semaphore>>,
}
struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl Tool for Probe {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "Concurrency fixture"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{},"additionalProperties":false})
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Host
    }
    fn concurrency(&self) -> ToolConcurrency {
        self.lane
    }
    async fn execute(&self, _: &str, context: &mut ToolContext<'_>) -> Result<String> {
        assert_eq!(
            self.active.load(Ordering::SeqCst),
            0,
            "barrier overlaps parallel tools"
        );
        self.started.send(context.tool_call_id.to_owned()).unwrap();
        context.state.insert("barrier".into(), json!(true));
        Ok("barrier".into())
    }
    async fn execute_parallel(&self, _: &str, context: &ParallelToolContext<'_>) -> Result<String> {
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(self.active.clone());
        self.started.send(context.tool_call_id.to_owned()).unwrap();
        self.gates[context.tool_call_id].acquire().await?.forget();
        if context.tool_call_id == "fail" {
            anyhow::bail!("fixture failure");
        }
        Ok(context.tool_call_id.into())
    }
}
fn call(name: &str, id: &str) -> ChatCompletionMessageToolCalls {
    serde_json::from_value(
        json!({"id":id,"type":"function","function":{"name":name,"arguments":"{}"}}),
    )
    .unwrap()
}
fn fixture(
    ids: &[&str],
    limit: usize,
) -> (
    Agent,
    Arc<HashMap<String, Semaphore>>,
    mpsc::UnboundedReceiver<String>,
    Arc<AtomicUsize>,
) {
    let gates: Arc<HashMap<String, Semaphore>> = Arc::new(
        ids.iter()
            .map(|id| (id.to_string(), Semaphore::new(0)))
            .collect(),
    );
    let active = Arc::new(AtomicUsize::new(0));
    let (started, receiver) = mpsc::unbounded_channel();
    let tools = [
        ("parallel", ToolConcurrency::Parallel),
        ("wait", ToolConcurrency::Wait),
        ("barrier", ToolConcurrency::Exclusive),
    ]
    .into_iter()
    .map(|(name, lane)| {
        (
            name.into(),
            Arc::new(Probe {
                name,
                lane,
                active: active.clone(),
                started: started.clone(),
                gates: gates.clone(),
            }) as Arc<dyn Tool>,
        )
    })
    .collect();
    let mut agent = Agent::new(
        "http://127.0.0.1:1".into(),
        "fixture".into(),
        "test",
        None::<String>,
        Arc::new(tools),
    );
    agent.max_parallel_calls = limit;
    (agent, gates, receiver, active)
}
fn session(agent: &Agent, calls: &[ChatCompletionMessageToolCalls]) -> Session {
    let mut session = Session::new();
    session.begin_execution("test concurrency").unwrap();
    agent.record_tool_calls(&mut session, calls).unwrap();
    session
}

#[tokio::test]
async fn parallel_calls_obey_limit_and_exclusive_barriers() {
    let (agent, gates, mut started, active) = fixture(&["a", "b", "c", "d"], 2);
    let calls = [
        call("parallel", "a"),
        call("parallel", "b"),
        call("parallel", "c"),
        call("barrier", "edit"),
        call("parallel", "d"),
    ];
    let mut session = session(&agent, &calls);
    tokio::time::timeout(Duration::from_secs(5), async {
        let execution = agent.execute_tool_calls(&mut session, &calls, None, None);
        let control = async {
            let mut first = vec![started.recv().await.unwrap(), started.recv().await.unwrap()];
            first.sort();
            assert_eq!(first, ["a", "b"]);
            assert_eq!(active.load(Ordering::SeqCst), 2);
            assert!(started.try_recv().is_err());
            gates["b"].add_permits(1);
            assert_eq!(started.recv().await.unwrap(), "c");
            gates["a"].add_permits(1);
            gates["c"].add_permits(1);
            assert_eq!(started.recv().await.unwrap(), "edit");
            assert_eq!(started.recv().await.unwrap(), "d");
            gates["d"].add_permits(1);
        };
        let (result, ()) = tokio::join!(execution, control);
        result.unwrap();
    })
    .await
    .unwrap();
    assert_eq!(session.state["barrier"], true);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    let completed: Vec<_> = session
        .events
        .iter()
        .flat_map(|event| &event.content)
        .filter_map(|item| {
            if let ContentItem::ToolResult { tool_call_id, .. } = item {
                Some(tool_call_id.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(completed[0], "b");
    let messages = serde_json::to_value(build_messages("fixture", &session).unwrap()).unwrap();
    let projected: Vec<_> = messages
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["tool_call_id"].as_str().unwrap())
        .collect();
    assert_eq!(projected, ["a", "b", "c", "edit", "d"]);
}

#[tokio::test]
async fn waiting_does_not_consume_command_slots_and_failure_is_local() {
    let (agent, gates, mut started, _) = fixture(&["a", "fail", "waiting"], 1);
    let calls = [
        call("parallel", "a"),
        call("parallel", "fail"),
        call("wait", "waiting"),
    ];
    let mut session = session(&agent, &calls);
    tokio::time::timeout(Duration::from_secs(5), async {
        let execution = agent.execute_tool_calls(&mut session, &calls, None, None);
        let control = async {
            let mut first = vec![started.recv().await.unwrap(), started.recv().await.unwrap()];
            first.sort();
            assert_eq!(first, ["a", "waiting"]);
            gates["waiting"].add_permits(1);
            gates["a"].add_permits(1);
            assert_eq!(started.recv().await.unwrap(), "fail");
            gates["fail"].add_permits(1);
        };
        let (result, ()) = tokio::join!(execution, control);
        result.unwrap();
    })
    .await
    .unwrap();
    let statuses: HashMap<_, _> = session
        .events
        .iter()
        .flat_map(|event| &event.content)
        .filter_map(|item| {
            if let ContentItem::ToolResult {
                tool_call_id,
                status,
                ..
            } = item
            {
                Some((tool_call_id.as_str(), *status))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(statuses["fail"], ToolResultStatus::Error);
    assert_eq!(statuses["a"], ToolResultStatus::Success);
    assert_eq!(statuses["waiting"], ToolResultStatus::Success);
}

#[tokio::test]
async fn dropping_dispatch_cancels_running_futures_without_starting_queued_calls() {
    let (agent, _, mut started, active) = fixture(&["a", "b"], 1);
    let calls = [call("parallel", "a"), call("parallel", "b")];
    let mut session = session(&agent, &calls);
    tokio::time::timeout(Duration::from_secs(5), async {
        let execution = agent.execute_tool_calls(&mut session, &calls, None, None);
        tokio::pin!(execution);
        tokio::select! {
            _ = &mut execution => panic!("blocked tool unexpectedly completed"),
            id = started.recv() => assert_eq!(id.unwrap(), "a"),
        }
    })
    .await
    .unwrap();
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert!(started.try_recv().is_err());
    assert!(
        session.current_execution().unwrap().tool_presentations["a"]
            .started_at
            .is_some()
    );
    assert!(
        session.current_execution().unwrap().tool_presentations["b"]
            .started_at
            .is_none()
    );
}

#[tokio::test]
async fn invalid_arguments_do_not_execute_or_prevent_valid_siblings() {
    let (agent, gates, mut started, _) = fixture(&["good"], 2);
    let mut invalid = call("parallel", "bad");
    if let ChatCompletionMessageToolCalls::Function(f) = &mut invalid {
        f.function.arguments = r#"{"unexpected":true}"#.into();
    }
    let calls = [invalid, call("parallel", "good")];
    gates["good"].add_permits(1);
    let mut session = session(&agent, &calls);
    agent
        .execute_tool_calls(&mut session, &calls, None, None)
        .await
        .unwrap();
    assert_eq!(started.try_recv().unwrap(), "good");
    assert!(started.try_recv().is_err());
    assert!(session.events.iter().flat_map(|e| &e.content).any(|item| matches!(item,
        ContentItem::ToolResult {tool_call_id,status:ToolResultStatus::Error,..} if tool_call_id=="bad")));
}

#[tokio::test]
async fn model_parallel_hint_is_optional_and_independent_of_execution_limit() {
    for hint in [None, Some(true), Some(false)] {
        let (url, requests, server) =
            super::server(vec![json!({"role":"assistant","content":"done"})]).await;
        let (mut agent, _, _, _) = fixture(&[], 1);
        agent.base_url = url;
        agent.parallel_tool_calls = hint;
        agent
            .run_in_session(&mut Session::new(), "test")
            .await
            .unwrap();
        server.await.unwrap();
        let requests = requests.lock().await;
        assert_eq!(
            requests[0]
                .get("parallel_tool_calls")
                .and_then(Value::as_bool),
            hint
        );
    }
}

#[tokio::test]
async fn completed_sibling_is_checkpointed_and_streamed_while_other_call_is_running() {
    let (agent, gates, mut started, _) = fixture(&["slow", "fast"], 2);
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::new(directory.path()).await.unwrap());
    let agent = agent.with_session_store(store.clone());
    let calls = [call("parallel", "slow"), call("parallel", "fast")];
    let mut session = session(&agent, &calls);
    let id = session.session_id.clone();
    let _lease = acquire_session(&id).unwrap();
    let (stream, mut events, _cancel) = super::super::events::compaction_test_stream();
    tokio::time::timeout(Duration::from_secs(5), async {
        let execution = agent.execute_tool_calls(&mut session, &calls, None, Some(&stream));
        let control = async {
            started.recv().await.unwrap();
            started.recv().await.unwrap();
            gates["fast"].add_permits(1);
            loop {
                if let Some(super::super::events::AgentStreamEvent::ToolFinished {
                    call_id, ..
                }) = events.recv().await
                {
                    assert_eq!(call_id, "fast");
                    let saved = store.load(&id).await.unwrap();
                    assert!(
                        saved
                            .events
                            .iter()
                            .flat_map(|e| &e.content)
                            .any(|item| matches!(item,
                        ContentItem::ToolResult {tool_call_id,..} if tool_call_id=="fast"))
                    );
                    assert!(
                        !saved
                            .events
                            .iter()
                            .flat_map(|e| &e.content)
                            .any(|item| matches!(item,
                        ContentItem::ToolResult {tool_call_id,..} if tool_call_id=="slow"))
                    );
                    break;
                }
            }
            gates["slow"].add_permits(1);
        };
        let (result, ()) = tokio::join!(execution, control);
        result.unwrap();
    })
    .await
    .unwrap();
}
