use std::{path::Path, time::Duration};

use axum::{Router, body::Body, response::Response, routing::post};
use serde_json::json;

use kyler_config::{AppConfig, SecretString};
use kyler_core::{
    runtime::UserTurn,
    thread::{ThreadEvent, ThreadManager},
};
use kyler_protocol::{
    api::{
        event::EventPayload,
        permissions::{PermissionMode, PermissionSelection},
        run::RunStatus,
    },
    sandbox::SandboxMode,
};

fn config(root: &Path, url: &str) -> AppConfig {
    let mut config = AppConfig::default();
    config.llm.base_url = url.into();
    config.llm.model = "fixture".into();
    config.llm.api_key = SecretString::new("fixture-only");
    config.session.directory = root.join("sessions");
    config.sandbox.mode = SandboxMode::ExternalSandbox;
    config.sandbox.external_sandbox_confirmed = true;
    config.skills.enabled = false;
    config.subagent.enabled = false;
    config.subagent.persist_sessions = false;
    config
}
async fn manager(config: AppConfig) -> ThreadManager {
    ThreadManager::new(config, None, std::env::current_exe().unwrap())
        .await
        .unwrap()
}
async fn fixture() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = Router::new().route("/v1/chat/completions", post(|| async {
        let chunk = json!({"id":"f","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":{"content":"local answer"},"finish_reason":null}]});
        let end = json!({"id":"f","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
        Response::builder().header("content-type","text/event-stream").body(Body::from(format!("data: {chunk}\n\ndata: {end}\n\ndata: [DONE]\n\n"))).unwrap()
    }));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, task)
}
#[tokio::test]
async fn local_thread_streams_persists_and_resumes_its_original_workspace() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let (url, fixture) = fixture().await;
    let config = config(root.path(), &url);
    let manager = manager(config.clone()).await;
    let new = manager.start_thread(root.path(), None).await.unwrap();
    let id = new.session_configured.session_id;
    let mut events = new.thread.take_events().await.unwrap();
    assert!(new.thread.take_events().await.is_err());
    new.thread
        .submit_turn(UserTurn {
            text: "hello".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match events.recv().await.unwrap() {
                ThreadEvent::Event(event) => {
                    if let EventPayload::TextDelta { text: delta, .. } = event.payload {
                        text.push_str(&delta);
                    }
                }
                ThreadEvent::TurnFinished(info) => {
                    assert_eq!(info.status, RunStatus::Completed, "{:?}", info.error);
                    assert_eq!(info.output.as_deref(), Some("local answer"));
                    break;
                }
                _ => panic!("unexpected shutdown"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(text, "local answer");
    assert!(!new.thread.history(0, 100).await.unwrap().events.is_empty());
    let second = ThreadManager::new(config.clone(), None, std::env::current_exe().unwrap())
        .await
        .unwrap();
    assert!(second.start_thread(root.path(), Some(&id)).await.is_err());
    second.shutdown().await.unwrap();
    drop(events);
    manager.shutdown().await.unwrap();
    let resumed_manager = ThreadManager::new(config, None, std::env::current_exe().unwrap())
        .await
        .unwrap();
    let resumed = resumed_manager
        .start_thread(other.path(), Some(&id))
        .await
        .unwrap();
    assert_eq!(
        resumed
            .session_configured
            .environment
            .unwrap()
            .working_directory,
        root.path().canonicalize().unwrap()
    );
    assert!(!resumed.thread.session().await.events.is_empty());
    resumed_manager.shutdown().await.unwrap();
    fixture.abort();
}
#[tokio::test]
async fn cancellation_is_processed_while_the_model_is_stalled_and_shutdown_joins() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let fixture = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let manager = manager(config(root.path(), &url)).await;
    let new = manager.start_thread(root.path(), None).await.unwrap();
    let mut events = new.thread.take_events().await.unwrap();
    new.thread
        .submit_turn(UserTurn {
            text: "wait".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        new.thread
            .submit_turn(UserTurn {
                text: "duplicate".into(),
                ..Default::default()
            })
            .await
            .is_err()
    );
    // The command loop stays responsive while the model stream awaits network input.
    tokio::time::timeout(Duration::from_secs(2), new.thread.cancel())
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            if let ThreadEvent::TurnFinished(info) = event {
                assert_eq!(info.status, RunStatus::Cancelled);
                return;
            }
        }
        panic!("missing terminal event");
    })
    .await
    .unwrap();
    drop(events);
    tokio::time::timeout(Duration::from_secs(5), manager.shutdown())
        .await
        .unwrap()
        .unwrap();
    fixture.abort();
}
#[tokio::test]
async fn local_permissions_cannot_expand_host_authority_and_preferences_survive_resume() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path(), "http://127.0.0.1:1/v1");
    let manager = manager(config.clone()).await;
    let new = manager.start_thread(root.path(), None).await.unwrap();
    let id = new.session_configured.session_id;
    assert!(
        new.thread
            .permissions(Some(&PermissionSelection {
                mode: Some(PermissionMode::FullAccess),
                approval: None,
                confirm_full_access: true
            }))
            .await
            .is_err()
    );
    new.thread
        .permissions(Some(&PermissionSelection {
            mode: Some(PermissionMode::ReadOnly),
            approval: None,
            confirm_full_access: false,
        }))
        .await
        .unwrap();
    manager.shutdown().await.unwrap();
    let manager = ThreadManager::new(config, None, std::env::current_exe().unwrap())
        .await
        .unwrap();
    let restored = manager.start_thread(root.path(), Some(&id)).await.unwrap();
    assert_eq!(
        restored
            .thread
            .permissions(None)
            .await
            .unwrap()
            .selected
            .mode,
        PermissionMode::ReadOnly
    );
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn tool_question_events_and_answers_flow_without_a_service() {
    use axum::Json;
    use kyler_protocol::api::questions::{QuestionResponse, QuestionStatus};
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = Router::new().route("/v1/chat/completions",post(|Json(body): Json<serde_json::Value>| async move {
        let answered = body["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool");
        let delta = if answered {json!({"content":"question handled"})} else {
            json!({"tool_calls":[{"index":0,"id":"question-call","type":"function","function":{"name":"ask_question","arguments":json!({"questions":[{"id":"choice","question":"Continue?","type":"text"}]}).to_string()}}]})
        };
        let chunk = json!({"id":"f","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":delta,"finish_reason":null}]});
        let end = json!({"id":"f","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":{},"finish_reason":if answered {"stop"} else {"tool_calls"}}]});
        Response::builder().header("content-type","text/event-stream").body(Body::from(format!("data: {chunk}\n\ndata: {end}\n\ndata: [DONE]\n\n"))).unwrap()
    }));
    let fixture = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let manager = manager(config(root.path(), &url)).await;
    let new = manager.start_thread(root.path(), None).await.unwrap();
    let id = new.session_configured.session_id;
    let mut events = new.thread.take_events().await.unwrap();
    new.thread
        .submit_turn(UserTurn {
            text: "ask me".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut queued = false;
    let mut finished = false;
    let mut answered = false;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match events.recv().await.unwrap() {
                ThreadEvent::Event(event) => match event.payload {
                    EventPayload::ToolQueued { .. } => queued = true,
                    EventPayload::ToolFinished { .. } => finished = true,
                    EventPayload::QuestionUpdated { question }
                        if question.status == QuestionStatus::Pending =>
                    {
                        new.thread
                            .respond_question(
                                id.clone(),
                                question.id.clone(),
                                QuestionResponse::Cancel,
                            )
                            .await
                            .unwrap();
                        answered = true;
                    }
                    _ => {}
                },
                ThreadEvent::TurnFinished(info) => {
                    assert_eq!(info.status, RunStatus::Completed, "{:?}", info.error);
                    break;
                }
                _ => panic!("unexpected shutdown"),
            }
        }
    })
    .await
    .unwrap();
    assert!(queued && finished && answered);
    drop(events);
    manager.shutdown().await.unwrap();
    fixture.abort();
}

#[tokio::test]
async fn cancellation_does_not_wait_for_a_full_event_queue() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = Router::new().route("/v1/chat/completions",post(|| async {
        let chunk = json!({"id":"f","object":"chat.completion.chunk","created":0,"model":"fixture","choices":[{"index":0,"delta":{"content":"x"},"finish_reason":null}]});
        let body = format!("data: {chunk}\n\n").repeat(2000) + "data: [DONE]\n\n";
        Response::builder().header("content-type","text/event-stream").body(Body::from(body)).unwrap()
    }));
    let fixture = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let manager = manager(config(root.path(), &url)).await;
    let new = manager.start_thread(root.path(), None).await.unwrap();
    let mut events = new.thread.take_events().await.unwrap();
    new.thread
        .submit_turn(UserTurn {
            text: "many deltas".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(event) = events.recv().await {
            if matches!(
                event,
                ThreadEvent::Event(kyler_core::thread::ThreadNotification {
                    payload: EventPayload::TextDelta { .. },
                    ..
                })
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    tokio::time::timeout(Duration::from_millis(200), new.thread.cancel())
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            if let ThreadEvent::TurnFinished(info) = event {
                assert_eq!(info.status, RunStatus::Cancelled);
                return;
            }
        }
        panic!("missing terminal event");
    })
    .await
    .unwrap();
    drop(events);
    manager.shutdown().await.unwrap();
    fixture.abort();
}

#[tokio::test]
async fn operation_receipts_reject_conflicting_reuse_without_executing_twice() {
    use kyler_core::thread::ThreadOp;
    let root = tempfile::tempdir().unwrap();
    let (url, fixture) = fixture().await;
    let manager = manager(config(root.path(), &url)).await;
    let new = manager.start_thread(root.path(), None).await.unwrap();
    let mut events = new.thread.take_events().await.unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    for _ in 0..2 {
        let (reply, receive) = tokio::sync::oneshot::channel();
        new.thread
            .submit(ThreadOp::SubmitTurn {
                op_id: id.clone(),
                turn: UserTurn {
                    text: "one operation".into(),
                    ..Default::default()
                },
                context: None,
                reply,
            })
            .await
            .unwrap();
        assert_eq!(receive.await.unwrap().unwrap(), id);
    }
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(event) = events.recv().await {
            if let ThreadEvent::TurnFinished(info) = event {
                assert_eq!(info.run_id, id);
                assert_eq!(info.status, RunStatus::Completed);
                return;
            }
        }
        panic!("missing completion");
    })
    .await
    .unwrap();
    let before = new.thread.session().await.events.len();
    let (reply, receive) = tokio::sync::oneshot::channel();
    new.thread
        .submit(ThreadOp::SubmitTurn {
            op_id: id,
            turn: UserTurn {
                text: "changed input".into(),
                ..Default::default()
            },
            context: None,
            reply,
        })
        .await
        .unwrap();
    assert!(receive.await.unwrap().is_err());
    assert_eq!(new.thread.session().await.events.len(), before);
    assert!(events.try_recv().is_err());
    drop(events);
    manager.shutdown().await.unwrap();
    fixture.abort();
}

#[tokio::test]
async fn closing_a_hosted_thread_does_not_shutdown_shared_host_resources() {
    use kyler_core::{
        Agent, SessionStore, attachments::AttachmentStore, processes::ProcessManager,
        questions::QuestionManager, thread::HostedThread,
    };
    use kyler_protocol::api::permissions::PermissionSnapshot;
    use std::sync::Arc;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(
        SessionStore::new(root.path().join("sessions"))
            .await
            .unwrap(),
    );
    let session = store.create().await.unwrap();
    let agent =
        Agent::from_config_with_store(&config(root.path(), "http://127.0.0.1:1/v1"), store.clone())
            .await
            .unwrap();
    let manager = ThreadManager::hosted(store);
    let handle = manager
        .attach(HostedThread {
            agent: agent.clone(),
            session: session.clone(),
            approvals: None,
            questions: Arc::new(
                QuestionManager::persistent(root.path().join("questions")).unwrap(),
            ),
            processes: Arc::new(ProcessManager::default()),
            attachments: Arc::new(AttachmentStore::new(root.path().join("attachments"))),
            default_approval: Default::default(),
            preferences: root.path().into(),
            permissions: PermissionSnapshot {
                mode: PermissionMode::WorkspaceAccess,
                approval: None,
                revision: "initial".into(),
            },
            profile: None,
        })
        .await
        .unwrap();
    let events = handle.take_events().await.unwrap();
    handle.model(Some(None)).await.unwrap();
    assert!(
        root.path()
            .join(format!("{}.model", session.session_id))
            .is_file()
    );
    drop(events);
    manager.close(&session.session_id).await.unwrap();
    assert!(
        agent
            .mcp_manager()
            .unwrap()
            .list(&session.session_id, None)
            .await
            .is_ok()
    );
    manager.shutdown().await.unwrap();
    agent.shutdown().await;
}
