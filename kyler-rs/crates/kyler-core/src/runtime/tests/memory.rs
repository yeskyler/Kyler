use super::{agent, server};
use crate::{
    SessionStore,
    context::ExecutionStatus,
    memory::{MemoryStatus, MemoryStore},
    rollout::RolloutPayload,
    subagent::{
        AgentKind, DelegateTaskArgs, SubagentManager, SubagentStatus, TaskOwner, WaitMode,
        review::tests::fixture,
    },
};

use std::sync::Arc;

use serde_json::json;

#[tokio::test]
async fn memory_task_uses_committed_parent_history_and_saves_only_candidates() {
    let (_workspace, environment) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let history = Arc::new(
        SessionStore::new(directory.path().join("sessions"))
            .await
            .unwrap(),
    );
    let memory = Arc::new(MemoryStore::new(directory.path().join("memory")));
    let mut parent = history.create().await.unwrap();
    parent.environment = Some(environment.session_environment().clone());
    parent
        .begin_execution("Prefer concise release notes.")
        .unwrap();
    parent
        .finish(ExecutionStatus::Completed, None, None)
        .unwrap();
    history.save(&mut parent).await.unwrap();

    let mut reader = history.rollout(&parent.session_id).await.unwrap();
    let seq = loop {
        let record = reader.next().await.unwrap().unwrap();
        if matches!(record.payload, RolloutPayload::EventRecorded { .. }) {
            break record.seq;
        }
    };
    parent
        .begin_execution("password: redaction-fixture")
        .unwrap();
    parent
        .finish(ExecutionStatus::Completed, None, None)
        .unwrap();
    history.save(&mut parent).await.unwrap();
    parent
        .begin_execution("Review memory; this turn must not be read.")
        .unwrap();
    history.save(&mut parent).await.unwrap();
    let owner = TaskOwner {
        session_id: parent.session_id.clone(),
        execution_id: parent.current_execution().unwrap().execution_id.clone(),
    };
    let report = json!({"candidates":[{
        "kind":"preference","content":"Prefer concise release notes.",
        "conditions":"When preparing a release.","evidence":[seq]
    }]});
    let (url, requests, model) = server(vec![json!({
        "role":"assistant","content":report.to_string()
    })])
    .await;
    let manager = Arc::new(SubagentManager::new(Default::default()).unwrap());
    let mut runtime = agent(url)
        .with_session_store(history)
        .with_environment(environment);
    runtime.memory_store = Some(memory.clone());
    let scope = manager
        .open_scope(owner, runtime.child_executor(&manager))
        .unwrap();
    let task = scope
        .delegate(
            DelegateTaskArgs {
                agent: AgentKind::Memory,
                review: None,
                task: "Review the committed session history".into(),
                context: Some("untrusted extra context".into()),
            },
            "memory-task",
        )
        .unwrap();
    let result = scope
        .wait(&[task.task_id], WaitMode::All, 10)
        .await
        .unwrap();
    let result = &result.tasks[0];
    assert_eq!(
        result.status,
        SubagentStatus::Completed,
        "{:?}",
        result.error
    );
    let entries = memory.list(&MemoryStore::scope(&parent)).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, MemoryStatus::Candidate);
    assert_eq!(entries[0].source_session, parent.session_id);
    assert_eq!(entries[0].source_tool_call.as_deref(), Some("memory-task"));
    assert!(
        memory
            .search(&MemoryStore::scope(&parent), "")
            .unwrap()
            .is_empty()
    );
    model.await.unwrap();
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert!(request["tools"].is_null() || request["tools"].as_array().is_some_and(Vec::is_empty));
    let messages = request["messages"].to_string();
    assert!(messages.contains("Prefer concise release notes."));
    assert!(!messages.contains("this turn must not be read"));
    assert!(!messages.contains("untrusted extra context"));
    assert!(!messages.contains("redaction-fixture"));
}
