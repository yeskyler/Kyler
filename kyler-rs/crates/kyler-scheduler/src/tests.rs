use crate::{Store, schedule};

use chrono::DateTime;

use kyler_protocol::api::{
    automations::{ScheduledRunStatus as Status, TaskConfig},
    models::ThinkingLevel,
    permissions::PermissionMode,
    session::WorkspaceMode,
};

pub(crate) fn config() -> TaskConfig {
    TaskConfig {
        name: "Daily review".into(),
        prompt: "Review changes".into(),
        cwd: "/project".into(),
        workspace_mode: WorkspaceMode::Project,
        cron: "* * * * *".into(),
        timezone: "UTC".into(),
        model_profile: Some("default".into()),
        thinking: ThinkingLevel::High,
        permission: PermissionMode::WorkspaceAccess,
        timeout_seconds: 60,
        wait_timeout_seconds: 30,
    }
}
fn instant(value: &str) -> i64 {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .timestamp_millis()
}
#[test]
fn timezone_dst_and_preview_are_deterministic() {
    assert_eq!(
        schedule::next(
            "0 9 * * *",
            "Asia/Shanghai",
            instant("2026-11-17T00:00:00Z")
        )
        .unwrap(),
        instant("2026-11-17T01:00:00Z")
    );
    assert_eq!(
        schedule::next(
            "30 2 * * *",
            "America/New_York",
            instant("2026-03-08T00:00:00Z")
        )
        .unwrap(),
        instant("2026-03-09T06:30:00Z")
    );
    // The second 01:30 during the autumn overlap is never scheduled again.
    let first = schedule::next(
        "30 1 * * *",
        "America/New_York",
        instant("2026-11-01T00:00:00Z"),
    )
    .unwrap();
    assert_eq!(first, instant("2026-11-01T05:30:00Z"));
    assert_eq!(
        schedule::next("30 1 * * *", "America/New_York", first).unwrap(),
        instant("2026-11-02T06:30:00Z")
    );
    assert_eq!(
        schedule::preview("* * * * *", "UTC", 0).unwrap(),
        vec![60000, 120000, 180000, 240000, 300000]
    );
    assert!(schedule::next("* * * * * *", "UTC", 0).is_err());
    assert!(schedule::next("* * * * *", "invalid", 0).is_err());
}
#[tokio::test]
async fn persistence_revision_and_idempotency() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("scheduler.sqlite3");
    let store = Store::open(&path, "sessions").await.unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let task = store
        .save_task(None, None, config(), false, 0, Some(id.clone()))
        .await
        .unwrap();
    assert_eq!(
        store
            .save_task(None, None, config(), false, 10, Some(id))
            .await
            .unwrap()
            .id,
        task.id
    );
    let mut changed = config();
    changed.name = "Changed".into();
    assert!(
        store
            .save_task(Some(&task.id), Some(99), changed.clone(), false, 10, None)
            .await
            .is_err()
    );
    let updated = store
        .save_task(Some(&task.id), Some(1), changed, false, 10, None)
        .await
        .unwrap();
    assert_eq!(updated.revision, 2);
    let run = store.enqueue(&task.id, "request", 20).await.unwrap();
    assert_eq!(
        store.enqueue(&task.id, "request", 30).await.unwrap().id,
        run.id
    );
    assert!(store.enqueue(&task.id, "other", 30).await.is_err());
    assert!(store.delete(&task.id, 2, 30).await.is_err());
    assert!(store.runs(&task.id, None, 0).await.is_err());
    store.cancel(&run.id, 40).await.unwrap();
    let second = store.enqueue(&task.id, "other", 50).await.unwrap();
    assert_ne!(run.session_id, second.session_id);
    store.cancel(&second.id, 60).await.unwrap();
    let page = store.runs(&task.id, None, 1).await.unwrap();
    assert_eq!(page.runs[0].id, second.id);
    assert_eq!(
        store
            .runs(&task.id, page.next_cursor.as_deref(), 1)
            .await
            .unwrap()
            .runs[0]
            .id,
        run.id
    );
    drop(store);
    let store = Store::open(&path, "sessions").await.unwrap();
    assert_eq!(store.task(&task.id).await.unwrap().config.name, "Changed");
    store.delete(&task.id, 2, 70).await.unwrap();
    assert!(store.tasks().await.unwrap().is_empty());
    assert_eq!(store.runs(&task.id, None, 20).await.unwrap().runs.len(), 2);
    assert!(Store::open(&path, "different-store").await.is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[tokio::test]
async fn lease_fencing_and_concurrent_cancellation() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("scheduler.sqlite3");
    let first = Store::open(&path, "sessions").await.unwrap();
    let second = Store::open(&path, "sessions").await.unwrap();
    let lease = first.acquire("first".into(), 0).await.unwrap();
    assert!(second.acquire("second".into(), 1).await.is_err());
    let task = first
        .save_task(None, None, config(), false, 0, None)
        .await
        .unwrap();
    let queued = first.enqueue(&task.id, "request", 1).await.unwrap();
    let mut claimed = first.claim(lease.clone(), 2, 1).await.unwrap().remove(0);
    assert_eq!(claimed.status, Status::Starting);
    assert!(first.claim(lease.clone(), 3, 1).await.unwrap().is_empty());
    first.cancel(&queued.id, 4).await.unwrap();
    claimed.status = Status::Running;
    first
        .save_run(claimed.clone(), lease.clone(), 5, false)
        .await
        .unwrap();
    assert!(first.runs(&task.id, None, 20).await.unwrap().runs[0].cancel_requested);
    let newer = second.acquire("second".into(), 30001).await.unwrap();
    assert!(first.heartbeat(lease.clone(), 30002).await.is_err());
    assert!(first.save_run(claimed, lease, 30002, false).await.is_err());
    second.heartbeat(newer, 30002).await.unwrap();
}
#[tokio::test]
async fn scheduled_overlap_and_offline_times_are_skipped() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("scheduler.sqlite3"), "sessions")
        .await
        .unwrap();
    let task = store
        .save_task(None, None, config(), true, 0, None)
        .await
        .unwrap();
    let lease = store.acquire("owner".into(), 60000).await.unwrap();
    store.due(lease.clone(), 60000, false).await.unwrap();
    store.due(lease.clone(), 60000, false).await.unwrap();
    assert_eq!(store.runs(&task.id, None, 20).await.unwrap().runs.len(), 1);
    store.heartbeat(lease.clone(), 80000).await.unwrap();
    store.heartbeat(lease.clone(), 100000).await.unwrap();
    store.heartbeat(lease.clone(), 120000).await.unwrap();
    store.due(lease.clone(), 120000, false).await.unwrap();
    let runs = store.runs(&task.id, None, 20).await.unwrap().runs;
    assert!(store.task(&task.id).await.unwrap().has_unfinished_run);
    assert_eq!(runs[0].status, Status::Skipped);
    assert_eq!(runs[1].status, Status::Queued);
    store.release(lease).await.unwrap();
    let lease = store.acquire("new-owner".into(), 600000).await.unwrap();
    store.due(lease, 600000, true).await.unwrap();
    let task = store.task(&task.id).await.unwrap();
    assert_eq!(task.next_run_at, Some(660000));
    assert_eq!(task.latest_run.unwrap().status, Status::Skipped);
}

#[tokio::test]
async fn cancellation_during_deferred_admission_cannot_return_to_queue() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("scheduler.sqlite3"), "sessions")
        .await
        .unwrap();
    let task = store
        .save_task(None, None, config(), false, 0, None)
        .await
        .unwrap();
    let run = store.enqueue(&task.id, "request", 1).await.unwrap();
    let lease = store.acquire("owner".into(), 1).await.unwrap();
    let mut claimed = store.claim(lease.clone(), 2, 1).await.unwrap().remove(0);
    store.cancel(&run.id, 3).await.unwrap();
    claimed.status = Status::Queued;
    store
        .save_run(claimed, lease.clone(), 4, false)
        .await
        .unwrap();
    assert_eq!(
        store.runs(&task.id, None, 20).await.unwrap().runs[0].status,
        Status::Cancelled
    );
    assert!(store.claim(lease, 5, 1).await.unwrap().is_empty());
}
