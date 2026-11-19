use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use agent_client_protocol::{self as acp, Channel, Client, schema::ProtocolVersion, v1::schema::*};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use tokio::sync::{Mutex, mpsc};

use kyler_acp::{Host, HostEvent, HostSession};
use kyler_protocol::api::{
    event::EventPayload,
    permissions::{ApprovalDecision, ApprovalResponse},
    run::{RunInfo, RunStatus},
};

#[derive(Default)]
struct FakeHost {
    sessions: Mutex<HashMap<String, mpsc::Sender<HostEvent>>>,
    decisions: Mutex<Vec<ApprovalDecision>>,
    count: AtomicUsize,
    shutdown: AtomicBool,
}
fn finished(session: &str, run: &str, status: RunStatus) -> RunInfo {
    RunInfo {
        permissions: None,
        model: None,
        run_id: run.into(),
        session_id: session.into(),
        execution_id: None,
        status,
        created_at: 0,
        finished_at: Some(1),
        last_event_seq: 0,
        output: None,
        usage: None,
        error: None,
    }
}
#[async_trait]
impl Host for FakeHost {
    async fn new_session(&self, _: &Path) -> Result<HostSession> {
        let id = format!("session-{}", self.count.fetch_add(1, Ordering::SeqCst));
        let (tx, events) = mpsc::channel(16);
        self.sessions.lock().await.insert(id.clone(), tx);
        Ok(HostSession { id, events })
    }
    async fn submit(&self, session: &str, text: String) -> Result<String> {
        let tx = self.sessions.lock().await.get(session).unwrap().clone();
        let id = "run".to_string();
        if text.trim() == "approve" {
            let approval = serde_json::from_value(json!({"id":"approval","run_id":id,"session_id":session,"origin_session_id":session,"execution_id":"execution","tool_call_id":"call","tool":"shell","operation":"execute","preview":"Build project","requires_user":true,"working_directory":null,"policy":{"mode":"workspace_access","revision":"revision"},"digest":"digest","status":"pending","reason":"Review required","created_at":0,"expires_at":300})).unwrap();
            tx.send(HostEvent::Update {
                run_id: id.clone(),
                payload: EventPayload::ApprovalUpdated { approval },
            })
            .await?;
            return Ok(id);
        }
        if text.trim() == "wait" {
            return Ok(id);
        }
        tx.send(HostEvent::Update {
            run_id: id.clone(),
            payload: EventPayload::TextDelta {
                step: 1,
                text: "hello".into(),
            },
        })
        .await?;
        tx.send(HostEvent::Finished(finished(
            session,
            &id,
            RunStatus::Completed,
        )))
        .await?;
        Ok(id)
    }
    async fn cancel(&self, session: &str) -> Result<()> {
        if let Some(tx) = self.sessions.lock().await.get(session) {
            let _ = tx
                .send(HostEvent::Finished(finished(
                    session,
                    "run",
                    RunStatus::Cancelled,
                )))
                .await;
        }
        Ok(())
    }
    async fn approve(&self, session: &str, _: String, response: ApprovalResponse) -> Result<()> {
        assert_eq!(response.digest, "digest");
        self.decisions.lock().await.push(response.decision);
        self.sessions
            .lock()
            .await
            .get(session)
            .unwrap()
            .send(HostEvent::Finished(finished(
                session,
                "run",
                RunStatus::Completed,
            )))
            .await?;
        Ok(())
    }
    async fn dismiss_question(&self, _: &str, _: String) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        self.shutdown.store(true, Ordering::SeqCst);
        Ok(())
    }
}
fn request(session: SessionId, text: &str) -> PromptRequest {
    PromptRequest::new(session, vec![ContentBlock::Text(TextContent::new(text))])
}

#[tokio::test]
async fn official_sdk_round_trip_guards_sessions_and_streams_before_completion() {
    let host = Arc::new(FakeHost::default());
    let (agent, client) = Channel::duplex();
    let task = tokio::spawn(kyler_acp::serve(host.clone(), agent));
    let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
    let capture = updates.clone();
    Client.v1().on_receive_notification(async move |notification:SessionNotification,_| {
        capture.lock().unwrap().push(notification);Ok(())
    },acp::on_receive_notification!()).connect_with(client,async |connection| {
        assert!(connection.send_request(NewSessionRequest::new("/tmp")).block_task().await.is_err());
        let initialized=connection.send_request(InitializeRequest::new(ProtocolVersion::V1)).block_task().await?;
        assert_eq!(initialized.protocol_version,ProtocolVersion::V1);
        assert!(!initialized.agent_capabilities.load_session);
        assert!(connection.send_request(NewSessionRequest::new("relative")).block_task().await.is_err());
        let session=connection.send_request(NewSessionRequest::new("/tmp")).block_task().await?.session_id;
        let result=connection.send_request(request(session.clone(),"hello")).block_task().await?;
        assert_eq!(result.stop_reason,StopReason::EndTurn);
        assert!(connection.send_request(request("other-connection".into(),"hello")).block_task().await.is_err());
        let invalid:PromptRequest=serde_json::from_value(json!({"sessionId":session,"prompt":[{"type":"image","data":"AA==","mimeType":"image/png"}]})).unwrap();
        assert!(connection.send_request(invalid).block_task().await.is_err());
        Ok(())
    }).await.unwrap();
    task.await.unwrap().unwrap();
    assert!(host.shutdown.load(Ordering::SeqCst));
    assert_eq!(updates.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cancellation_is_processed_while_prompt_is_pending_and_other_sessions_progress() {
    let host = Arc::new(FakeHost::default());
    let (agent, client) = Channel::duplex();
    let task = tokio::spawn(kyler_acp::serve(host.clone(), agent));
    Client
        .v1()
        .on_receive_notification(
            async |_: SessionNotification, _| Ok(()),
            acp::on_receive_notification!(),
        )
        .connect_with(client, async |connection| {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let first = connection
                .send_request(NewSessionRequest::new("/tmp"))
                .block_task()
                .await?
                .session_id;
            let second = connection
                .send_request(NewSessionRequest::new("/tmp"))
                .block_task()
                .await?
                .session_id;
            let pending = connection.send_request(request(first.clone(), "wait"));
            // A subsequent request on the same connection must not be blocked by the pending prompt.
            let completed = connection
                .send_request(request(second, "hello"))
                .block_task()
                .await?;
            assert_eq!(completed.stop_reason, StopReason::EndTurn);
            assert!(
                connection
                    .send_request(request(first.clone(), "duplicate"))
                    .block_task()
                    .await
                    .is_err()
            );
            connection.send_notification(CancelNotification::new(first))?;
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(3), pending.block_task())
                    .await
                    .unwrap()?;
            assert_eq!(result.stop_reason, StopReason::Cancelled);
            Ok(())
        })
        .await
        .unwrap();
    task.await.unwrap().unwrap();
    assert!(host.shutdown.load(Ordering::SeqCst));
}

#[tokio::test]
async fn dropping_the_client_cleans_up_an_active_prompt() {
    let host = Arc::new(FakeHost::default());
    let (agent, client) = Channel::duplex();
    let task = tokio::spawn(kyler_acp::serve(host.clone(), agent));
    Client
        .v1()
        .connect_with(client, async |connection| {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(NewSessionRequest::new("/tmp"))
                .block_task()
                .await?
                .session_id;
            let _pending = connection.send_request(request(session, "wait"));
            tokio::task::yield_now().await;
            Ok(())
        })
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(host.shutdown.load(Ordering::SeqCst));
}

#[tokio::test]
async fn permission_rpc_maps_exact_options_and_rejects_forged_persistent_access() {
    for (option, expected) in [
        ("allow_once", ApprovalDecision::AllowOnce),
        ("allow_always", ApprovalDecision::Deny),
    ] {
        let host = Arc::new(FakeHost::default());
        let (agent, client) = Channel::duplex();
        let task = tokio::spawn(kyler_acp::serve(host.clone(), agent));
        Client
            .v1()
            .on_receive_request(
                async move |request: RequestPermissionRequest, responder, _| {
                    assert_eq!(request.options.len(), 2);
                    let response: RequestPermissionResponse = serde_json::from_value(
                        json!({"outcome":{"outcome":"selected","optionId":option}}),
                    )
                    .unwrap();
                    responder.respond(response)
                },
                acp::on_receive_request!(),
            )
            .connect_with(client, async |connection| {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session = connection
                    .send_request(NewSessionRequest::new("/tmp"))
                    .block_task()
                    .await?
                    .session_id;
                let result = connection
                    .send_request(request(session, "approve"))
                    .block_task()
                    .await?;
                assert_eq!(result.stop_reason, StopReason::EndTurn);
                Ok(())
            })
            .await
            .unwrap();
        task.await.unwrap().unwrap();
        assert_eq!(*host.decisions.lock().await, vec![expected]);
    }
}
