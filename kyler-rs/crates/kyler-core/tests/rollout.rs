use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use serde_json::json;

use kyler_core::{
    RolloutPayload, RolloutReader, RolloutRecord, RolloutWriter, Session, SessionStore,
    context::{
        ContentItem, Event, EventAuthor, ExecutionPlan, ExecutionStatus, PlanStep, PlanStepStatus,
    },
    rollout::replay::replay,
};

async fn records(store: &SessionStore, id: &str) -> Vec<RolloutRecord> {
    let mut reader = store.rollout(id).await.unwrap();
    let mut records = Vec::new();
    while let Some(record) = reader.next().await.unwrap() {
        records.push(record);
    }
    records
}
fn append(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

#[tokio::test]
async fn delta_history_replays_exact_state_and_preserves_original_tool_output() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).await.unwrap();
    let mut session = store.create().await.unwrap();
    let id = session.session_id.clone();
    session.begin_execution("first question").unwrap();
    session.current_execution_mut().unwrap().model = Some("test-model".into());
    session.current_execution_mut().unwrap().plan = Some(ExecutionPlan {
        summary: "work".into(),
        steps: vec![PlanStep {
            id: "one".into(),
            content: "do it".into(),
            status: PlanStepStatus::InProgress,
        }],
    });
    session.state.insert("remove".into(), json!(true));
    store.save(&mut session).await.unwrap();
    let execution_id = session.current_execution().unwrap().execution_id.clone();
    session.add_event(Event::new(
        &execution_id,
        EventAuthor {
            name: "agent".into(),
        },
        vec![ContentItem::ToolCall {
            tool_call_id: "c".into(),
            name: "read".into(),
            arguments: json!({}),
        }],
    ));
    store.save(&mut session).await.unwrap();
    session.add_event(Event::new(
        &execution_id,
        EventAuthor {
            name: "agent".into(),
        },
        vec![ContentItem::ToolResult {
            tool_call_id: "c".into(),
            name: "read".into(),
            status: kyler_core::context::ToolResultStatus::Success,
            content: "original long output".into(),
            model_content: Some("compact".into()),
        }],
    ));
    session.memory.summary = Some("summary".into());
    session.memory.compression_count = 1;
    session.add_usage(10, 2, 12);
    session.state.remove("remove");
    session.state.insert("keep".into(), json!({"a":1}));
    session
        .finish(ExecutionStatus::Completed, Some("done".into()), None)
        .unwrap();
    store.save(&mut session).await.unwrap();
    session.begin_execution("followup").unwrap();
    session
        .finish(ExecutionStatus::Completed, Some("again".into()), None)
        .unwrap();
    store.save(&mut session).await.unwrap();
    let list = records(&store, &id).await;
    assert!(list.iter().enumerate().all(|(i, r)| r.seq == i as u64 + 1));
    assert_eq!(
        list.iter()
            .filter(|r| matches!(r.payload, RolloutPayload::EventRecorded { .. }))
            .count(),
        session.events.len()
    );
    assert!(
        list.iter()
            .any(|r| matches!(r.payload, RolloutPayload::ContextCompacted { .. }))
    );
    assert!(
        list.iter()
            .any(|r| matches!(r.payload, RolloutPayload::ExecutionArchived { .. }))
    );
    let mut reader = store.rollout(&id).await.unwrap();
    let restored = replay(&mut reader).await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&restored).unwrap(),
        serde_json::to_value(&session).unwrap()
    );
    fs::remove_file(dir.path().join(&id).join("snapshot.json")).unwrap();
    let loaded = store.load(&id).await.unwrap();
    assert_eq!(
        serde_json::to_value(loaded).unwrap(),
        serde_json::to_value(session).unwrap()
    );
}

#[tokio::test]
async fn unfinished_transaction_is_hidden_and_truncated_before_next_save() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).await.unwrap();
    let mut session = store.create().await.unwrap();
    let id = session.session_id.clone();
    let log = dir.path().join(&id).join("rollout.jsonl");
    let original = fs::read(&log).unwrap();
    let list = records(&store, &id).await;
    let pending = RolloutRecord {
        schema_version: 1,
        session_id: id.clone(),
        execution_id: None,
        seq: list.last().unwrap().seq + 1,
        timestamp_ms: 0,
        payload: RolloutPayload::StateUpdated {
            set: HashMap::from([("uncommitted".into(), json!(true))]),
            removed: vec![],
        },
    };
    append(
        &log,
        format!("{}\n{{\"partial", serde_json::to_string(&pending).unwrap()).as_bytes(),
    );
    let mut reader = RolloutReader::open(&log, &id).unwrap();
    let restored = replay(&mut reader).await.unwrap().unwrap();
    assert!(!restored.state.contains_key("uncommitted"));
    let loaded = store.load(&id).await.unwrap();
    assert_eq!(loaded.revision, session.revision);
    assert_eq!(fs::read(&log).unwrap(), original);
    session.state.insert("committed".into(), json!(true));
    store.save(&mut session).await.unwrap();
    let loaded = store.load(&id).await.unwrap();
    assert_eq!(loaded.state["committed"], true);
}

#[tokio::test]
async fn complete_corruption_and_sequence_gaps_are_errors_without_truncation() {
    for gap in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).await.unwrap();
        let session = store.create().await.unwrap();
        let id = &session.session_id;
        let log = dir.path().join(id).join("rollout.jsonl");
        if gap {
            let record = RolloutRecord {
                schema_version: 1,
                session_id: id.clone(),
                execution_id: None,
                seq: 999,
                timestamp_ms: 0,
                payload: RolloutPayload::CheckpointCommitted {
                    revision: 2,
                    updated_at: 0,
                },
            };
            append(
                &log,
                format!("{}\n", serde_json::to_string(&record).unwrap()).as_bytes(),
            );
        } else {
            append(&log, b"{broken}\n");
        }
        let bytes = fs::read(&log).unwrap();
        assert!(store.load(id).await.is_err());
        assert_eq!(fs::read(log).unwrap(), bytes);
    }
}

#[tokio::test]
async fn stale_or_invalid_snapshot_falls_back_to_journal_but_missing_log_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).await.unwrap();
    let mut session = store.create().await.unwrap();
    let id = session.session_id.clone();
    let snapshot = dir.path().join(&id).join("snapshot.json");
    let stale = fs::read(&snapshot).unwrap();
    session.state.insert("new".into(), json!(42));
    store.save(&mut session).await.unwrap();
    fs::write(&snapshot, &stale).unwrap();
    assert_eq!(store.load(&id).await.unwrap().state["new"], 42);
    fs::write(&snapshot, b"invalid cache").unwrap();
    assert_eq!(store.load(&id).await.unwrap().revision, session.revision);
    fs::remove_file(dir.path().join(&id).join("rollout.jsonl")).unwrap();
    assert!(store.load(&id).await.is_err());
}

#[tokio::test]
async fn legacy_migration_is_lossless_deduplicated_and_original_is_retained() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).await.unwrap();
    let mut legacy = Session::new();
    legacy.revision = 7;
    legacy.begin_execution("old question").unwrap();
    legacy
        .finish(ExecutionStatus::Completed, Some("answer".into()), None)
        .unwrap();
    legacy.state.insert("legacy".into(), json!(true));
    let path = dir.path().join(format!("{}.json", legacy.session_id));
    let bytes = serde_json::to_vec(&legacy).unwrap();
    fs::write(&path, &bytes).unwrap();
    let migrated = store.load(&legacy.session_id).await.unwrap();
    assert_eq!(migrated.revision, 8);
    assert_eq!(migrated.events.len(), 1);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(store.list().await.unwrap().len(), 1);
    let mut reader = store.rollout(&legacy.session_id).await.unwrap();
    assert_eq!(
        serde_json::to_value(replay(&mut reader).await.unwrap().unwrap()).unwrap(),
        serde_json::to_value(migrated).unwrap()
    );
    store.delete(&legacy.session_id).await.unwrap();
    assert!(!path.exists());
    assert!(store.load(&legacy.session_id).await.is_err());
}

#[tokio::test]
async fn existing_history_is_immutable_and_readers_have_a_fixed_committed_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).await.unwrap();
    let mut session = store.create().await.unwrap();
    let mut reader = store.rollout(&session.session_id).await.unwrap();
    session.begin_execution("question").unwrap();
    session
        .finish(ExecutionStatus::Completed, Some("ok".into()), None)
        .unwrap();
    store.save(&mut session).await.unwrap();
    assert_eq!(replay(&mut reader).await.unwrap().unwrap().revision, 1);
    let before = records(&store, &session.session_id).await.len();
    if let ContentItem::Message { content, .. } = &mut session.events[0].content[0] {
        *content = "rewritten".into();
    }
    assert!(store.save(&mut session).await.is_err());
    assert_eq!(records(&store, &session.session_id).await.len(), before);
}

#[test]
fn writer_rejects_two_owners_and_requires_commit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout.jsonl");
    let mut writer = RolloutWriter::open(&path, "id").unwrap();
    assert!(RolloutWriter::open(&path, "id").is_err());
    assert!(writer.append_checkpoint(None, vec![]).is_err());
    drop(writer);
    assert!(RolloutWriter::open(&path, "id").is_ok());
}

#[tokio::test]
async fn reader_cursor_advances_only_after_delivering_commit_and_files_are_private() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).await.unwrap();
    let session = store.create().await.unwrap();
    let directory = dir.path().join(&session.session_id);
    let mut reader = store.rollout(&session.session_id).await.unwrap();
    assert_eq!(reader.position().last_seq, 0);
    assert!(matches!(
        reader.next().await.unwrap().unwrap().payload,
        RolloutPayload::SessionCreated { .. }
    ));
    assert_eq!(reader.position().last_seq, 0);
    assert!(matches!(
        reader.next().await.unwrap().unwrap().payload,
        RolloutPayload::CheckpointCommitted { .. }
    ));
    assert_eq!(reader.position().last_seq, 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for file in ["rollout.jsonl", "snapshot.json"] {
            assert_eq!(
                fs::metadata(directory.join(file))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
