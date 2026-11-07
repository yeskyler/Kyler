mod support;
use kyler_config::ServerSettings;
use kyler_protocol::api::{
    processes::{ProcessInfo, ProcessMetadata, ProcessOutput},
    tasks::TaskInfo,
};
use support::Harness;

#[tokio::test]
async fn task_history_is_scoped_bounded_and_restored_after_restart() {
    let mut h = Harness::new(ServerSettings::default()).await;
    let session = h.session().await;
    let other = h.session().await;
    let id = uuid::Uuid::new_v4().to_string();
    let info = ProcessInfo {
        id: id.clone(),
        status: "completed".into(),
        cursor: 1,
        oldest_cursor: 1,
        metadata: Some(ProcessMetadata {
            command: "cargo test".into(),
            working_directory: "/project".into(),
            started_at: 100,
            finished_at: Some(200),
            exit_code: Some(1),
        }),
        output: vec![ProcessOutput {
            process_id: id.clone(),
            session_id: session.clone(),
            cursor: 1,
            stream: "stderr".into(),
            text: "error\n".repeat(5000),
        }],
        result: Some("PRIVATE_RESULT".into()),
        error: None,
    };
    let directory = h.store.directory().join("_processes");
    std::fs::write(
        directory.join(format!("{id}.json")),
        serde_json::to_vec(&(&session, &info)).unwrap(),
    )
    .unwrap();
    let agent_id = uuid::Uuid::new_v4().to_string();
    let child_id = uuid::Uuid::new_v4().to_string();
    let child_session = uuid::Uuid::new_v4().to_string();
    let mut child = info.clone();
    child.id = child_id.clone();
    child.status = "running".into();
    std::fs::write(
        directory.join(format!("{child_id}.json")),
        serde_json::to_vec(&(&child_session, &child)).unwrap(),
    )
    .unwrap();
    let mut saved = h.store.load(&session).await.unwrap();
    saved.subagent_tasks.insert(
        agent_id.clone(),
        kyler_core::subagent::SubagentTask {
            author: Default::default(),
            agent: Default::default(),
            review: None,
            task_id: agent_id.clone(),
            parent_session_id: session.clone(),
            parent_execution_id: "execution".into(),
            parent_tool_call_id: "call".into(),
            session_id: child_session,
            execution_id: None,
            status: kyler_core::subagent::SubagentStatus::Running,
            output: Some("x".repeat(40000)),
            error: None,
            usage: Default::default(),
            created_at: 100,
            started_at: Some(100),
            finished_at: None,
        },
    );
    h.store.save(&mut saved).await.unwrap();
    h.restart().await;
    let tasks: Vec<TaskInfo> = h
        .client
        .get(format!("{}/v1/sessions/{session}/tasks", h.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tasks.len(), 2);
    let command = tasks.iter().find(|task| task.id == id).unwrap();
    assert_eq!(command.name, "cargo test");
    assert_eq!(command.status, "failed");
    assert!(tasks.iter().all(|task| task.output.is_none()));
    let agent = tasks.iter().find(|task| task.id == agent_id).unwrap();
    assert_eq!(agent.status, "interrupted");
    assert_eq!(agent.children.len(), 1);
    assert_eq!(agent.children[0].id, child_id);
    assert_eq!(agent.children[0].status, "interrupted");
    let agent: TaskInfo = h
        .client
        .get(format!(
            "{}/v1/sessions/{session}/tasks/agent/{agent_id}",
            h.url
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(agent.output_truncated && agent.output.unwrap().len() == 32000);
    let nested: TaskInfo = h
        .client
        .get(format!(
            "{}/v1/sessions/{session}/tasks/command/{child_id}",
            h.url
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(nested.id, child_id);
    let detail: TaskInfo = h
        .client
        .get(format!(
            "{}/v1/sessions/{session}/tasks/command/{id}",
            h.url
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(detail.output_truncated);
    assert_eq!(detail.output.unwrap().len(), 16000);
    assert_eq!(detail.exit_code, Some(1));
    for method in [reqwest::Method::GET, reqwest::Method::POST] {
        assert_eq!(
            h.client
                .request(
                    method,
                    format!("{}/v1/sessions/{other}/tasks/command/{id}", h.url)
                )
                .json(&serde_json::json!({"action":"stop"}))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }
    assert_eq!(
        h.client
            .post(format!(
                "{}/v1/sessions/{session}/tasks/command/{id}",
                h.url
            ))
            .json(&serde_json::json!({"action":"input","data":"x".repeat(4097)}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    h.close().await;
}
