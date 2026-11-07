//! Session storage transactions, optimistic revisions and live-execution recovery.
mod journal;
mod locking;

use crate::{
    rollout::{RolloutReader, RolloutWriter, writer::private_file},
    session::{Session, snapshot::sync_directory},
};
use journal::{commit, persist, read_locked};
use locking::{execution_lease, owned_executions, store_lock};

use std::{
    collections::BTreeSet,
    fs as disk,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tokio::{fs, sync::Mutex};
use uuid::Uuid;

pub(crate) use locking::{StoreExecutionLease, acquire_session};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub event_count: usize,
    pub execution_count: usize,
}

#[derive(Clone)]
pub struct SessionStore {
    root: Arc<PathBuf>,
    store_lock: Arc<Mutex<()>>,
}

fn validate_id(id: &str) -> Result<()> {
    ensure!(
        Uuid::parse_str(id)
            .context("invalid session ID")?
            .to_string()
            == id,
        "session ID must be a canonical UUID"
    );
    Ok(())
}

impl SessionStore {
    pub fn directory(&self) -> &Path {
        &self.root
    }

    pub async fn new(root: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(root.as_ref()).await?;
        let root = fs::canonicalize(root.as_ref()).await?;
        let store_lock = store_lock(&root);
        Ok(Self {
            root: Arc::new(root),
            store_lock,
        })
    }

    /// Keep both the Tokio mutex and OS lock in the blocking task, even if its caller
    /// is cancelled. A checkpoint cannot continue writing after releasing its lock.
    async fn transaction<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&Path) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let guard = self.store_lock.clone().lock_owned().await;
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let file = private_file(&root.join(".store.lock"), false)?;
            file.lock()?;
            operation(&root)
        })
        .await?
    }

    pub async fn create(&self) -> Result<Session> {
        let mut session = Session::new();
        self.save(&mut session).await?;
        Ok(session)
    }

    pub async fn load(&self, id: &str) -> Result<Session> {
        let id = id.to_owned();
        self.transaction(move |root| {
            let loaded = read_locked(root, &id)?.context("session not found")?;
            let legacy = loaded.writer.is_none();
            let mut session = loaded.session.clone();
            let local = acquire_session(&id).ok();
            let external = if local.is_some() {
                execution_lease(root, &id).ok()
            } else {
                None
            };
            let recover = session.is_running() && local.is_some() && external.is_some();
            if recover {
                session.recover_interrupted()?;
            }
            if legacy || recover {
                commit(root, &mut session, Some(loaded))?;
            }
            Ok(session)
        })
        .await
    }

    pub async fn save(&self, session: &mut Session) -> Result<()> {
        let snapshot = session.clone();
        let saved = self
            .transaction(move |root| persist(root, snapshot))
            .await?;
        session.revision = saved.revision;
        session.updated_at = saved.updated_at;
        Ok(())
    }

    pub(crate) async fn ensure_current(&self, session: &Session) -> Result<StoreExecutionLease> {
        let session = session.clone();
        self.transaction(move |root| {
            session.validate()?;
            let mut lease = execution_lease(root, &session.session_id)?;
            match read_locked(root, &session.session_id)? {
                Some(saved) => ensure!(
                    session.revision == saved.session.revision,
                    "session changed on disk; reload before saving or running"
                ),
                None => ensure!(
                    session.revision == 0,
                    "session was deleted; refusing to recreate stale session"
                ),
            }
            let owner = Arc::new(());
            let mut registry = owned_executions().lock().unwrap_or_else(|e| e.into_inner());
            registry.retain(|_, value| value.strong_count() > 0);
            registry.insert(root.join(&session.session_id), Arc::downgrade(&owner));
            lease._owner = Some(owner);
            Ok(lease)
        })
        .await
    }

    /// Stable committed-prefix reader; no model calls or tool execution during replay.
    pub async fn rollout(&self, id: &str) -> Result<RolloutReader> {
        let id = id.to_owned();
        self.transaction(move |root| {
            let loaded = read_locked(root, &id)?.context("session not found")?;
            if loaded.writer.is_none() {
                let mut session = loaded.session.clone();
                commit(root, &mut session, Some(loaded))?;
            }
            RolloutReader::open(root.join(&id).join("rollout.jsonl"), &id)
        })
        .await
    }

    pub async fn list(&self) -> Result<Vec<SessionInfo>> {
        self.transaction(|root| {
            let mut ids = BTreeSet::new();
            for entry in disk::read_dir(root)? {
                let path = entry?.path();
                let candidate = if path.is_dir() {
                    path.file_name()
                } else if path.extension().and_then(|s| s.to_str()) == Some("json") {
                    path.file_stem()
                } else {
                    None
                };
                if let Some(id) = candidate.and_then(|s| s.to_str()) {
                    if validate_id(id).is_ok() {
                        ids.insert(id.to_owned());
                    }
                }
            }
            let mut result = Vec::new();
            for id in ids {
                let Some(loaded) = read_locked(root, &id)? else {
                    continue;
                };
                let s = loaded.session;
                result.push(SessionInfo {
                    session_id: s.session_id,
                    created_at: s.created_at,
                    updated_at: s.updated_at,
                    event_count: s.events.len(),
                    execution_count: s.executions.len() + usize::from(s.execution.is_some()),
                });
            }
            result.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| a.session_id.cmp(&b.session_id))
            });
            Ok(result)
        })
        .await
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        let local = acquire_session(id)?;
        let id = id.to_owned();
        self.transaction(move |root| {
            let _local = local;
            let _external = execution_lease(root, &id)?;
            let directory = root.join(&id);
            let legacy = root.join(format!("{id}.json"));
            ensure!(directory.exists() || legacy.exists(), "session not found");
            // Remove legacy first so deleting the journal cannot resurrect an imported file.
            if legacy.exists() {
                disk::remove_file(legacy)?;
            }
            if directory.exists() {
                let _writer = if directory.join("rollout.jsonl").exists() {
                    Some(RolloutWriter::lock(&directory.join("rollout.jsonl"), &id)?)
                } else {
                    None
                };
                disk::remove_dir_all(directory)?;
            }
            sync_directory(root)?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionStore, locking::acquire_session};
    use crate::context::{ContentItem, Event, EventAuthor, ExecutionStatus};

    use std::path::PathBuf;

    use tokio::fs;
    use uuid::Uuid;

    fn directory() -> PathBuf {
        std::env::temp_dir().join(format!("kyler-sessions-{}", Uuid::new_v4()))
    }
    #[tokio::test]
    async fn agent_author_survives_rollout_replay_without_snapshot() {
        let dir = directory();
        let manager = SessionStore::new(&dir).await.unwrap();
        let mut session = crate::session::Session::new();
        assert_eq!(session.author.name, "kyler");
        session.author.name = "nova".into();
        manager.save(&mut session).await.unwrap();
        let snapshot = dir.join(&session.session_id).join("snapshot.json");
        if snapshot.exists() {
            fs::remove_file(snapshot).await.unwrap();
        }
        let loaded = manager.load(&session.session_id).await.unwrap();
        assert_eq!(loaded.author.name, "nova");
        let mut legacy = serde_json::to_value(&loaded).unwrap();
        legacy.as_object_mut().unwrap().remove("author");
        let legacy: crate::session::Session = serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy.author.name, "kyler");
        fs::remove_dir_all(dir).await.unwrap();
    }
    #[tokio::test]
    async fn persistence_roundtrip_list_delete_and_conflicts() {
        let dir = directory();
        let manager = SessionStore::new(&dir).await.unwrap();
        let mut session = manager.create().await.unwrap();
        session.begin_execution("remember this").unwrap();
        session
            .state
            .insert("name".into(), serde_json::json!("Kyler"));
        session.add_usage(12, 3, 15);
        session
            .finish(ExecutionStatus::Completed, Some("ok".into()), None)
            .unwrap();
        manager.save(&mut session).await.unwrap();
        let mut loaded = manager.load(&session.session_id).await.unwrap();
        assert_eq!(loaded.state["name"], "Kyler");
        assert_eq!(loaded.usage.total_tokens, 15);
        assert_eq!(loaded.current_execution().unwrap().usage.total_tokens, 15);
        assert_eq!(manager.list().await.unwrap()[0].event_count, 1);
        manager.save(&mut loaded).await.unwrap();
        assert!(manager.save(&mut session).await.is_err());
        manager.delete(&loaded.session_id).await.unwrap();
        assert!(manager.save(&mut loaded).await.is_err());
        assert!(manager.list().await.unwrap().is_empty());
        assert!(manager.load("../invalid").await.is_err());
        fs::remove_dir_all(dir).await.unwrap();
    }
    #[tokio::test]
    async fn load_repairs_orphaned_call_but_does_not_recover_live_execution() {
        let dir = directory();
        let manager = SessionStore::new(&dir).await.unwrap();
        let mut session = manager.create().await.unwrap();
        session.begin_execution("write data").unwrap();
        session.add_event(Event::new(
            session.current_execution().unwrap().execution_id.clone(),
            EventAuthor {
                name: "agent".into(),
            },
            vec![ContentItem::ToolCall {
                tool_call_id: "pending".into(),
                name: "write".into(),
                arguments: serde_json::json!({}),
            }],
        ));
        manager.save(&mut session).await.unwrap();
        let lease = acquire_session(&session.session_id).unwrap();
        let live = manager.load(&session.session_id).await.unwrap();
        assert!(live.is_running());
        assert!(manager.delete(&session.session_id).await.is_err());
        assert!(acquire_session(&session.session_id).is_err());
        drop(lease);
        let recovered = manager.load(&session.session_id).await.unwrap();
        assert_eq!(
            recovered.current_execution().unwrap().status,
            ExecutionStatus::Cancelled
        );
        assert!(matches!(
            recovered.events.last().unwrap().content[0],
            ContentItem::ToolResult { .. }
        ));
        assert!(recovered.events.last().unwrap().content.iter().any(
            |c| matches!(c, ContentItem::ToolResult { content, .. } if content.contains("unknown"))
        ));
        let second = manager.load(&session.session_id).await.unwrap();
        assert_eq!(second.events.len(), recovered.events.len());
        fs::remove_dir_all(dir).await.unwrap();
    }
    #[tokio::test]
    async fn independent_managers_share_store_lock_and_detect_stale_writes() {
        let dir = directory();
        let a = SessionStore::new(&dir).await.unwrap();
        let b = SessionStore::new(&dir).await.unwrap();
        let session = a.create().await.unwrap();
        let mut first = session.clone();
        let mut second = session;
        let (x, y) = tokio::join!(a.save(&mut first), b.save(&mut second));
        assert_ne!(x.is_ok(), y.is_ok());
        fs::remove_dir_all(dir).await.unwrap();
    }
}

#[cfg(test)]
mod process_tests {
    use super::{SessionStore, locking::acquire_session};
    use crate::{context::ExecutionStatus, rollout::RolloutWriter, session::Session};

    use std::{fs as disk, path::Path, process::Stdio};

    // Invoked in a fresh test process: it has no access to the parent's Tokio registries.
    #[tokio::test]
    async fn subprocess_probe() {
        let Ok(root) = std::env::var("KYLER_ROLLOUT_TEST_ROOT") else {
            return;
        };
        let mode = std::env::var("KYLER_ROLLOUT_TEST_MODE").unwrap();
        let id = std::env::var("KYLER_ROLLOUT_TEST_ID").unwrap();
        let store = SessionStore::new(&root).await.unwrap();
        if mode == "active" {
            let mut session = store.load(&id).await.unwrap();
            assert_eq!(
                session.current_execution().unwrap().status,
                ExecutionStatus::Running
            );
            assert!(store.ensure_current(&session).await.is_err());
            assert!(store.save(&mut session).await.is_err());
            assert!(store.delete(&id).await.is_err());
            // A local execution lease alone must not bypass another process's OS lease.
            let _local = acquire_session(&id).unwrap();
            assert!(store.save(&mut session).await.is_err());
        } else if mode == "writer" {
            assert!(
                RolloutWriter::open(Path::new(&root).join(&id).join("rollout.jsonl"), &id).is_err()
            );
        } else {
            let label = std::env::var("KYLER_ROLLOUT_TEST_LABEL").unwrap();
            let mut session: Session = serde_json::from_slice(
                &disk::read(Path::new(&root).join("race-input.json")).unwrap(),
            )
            .unwrap();
            while !Path::new(&root).join("start").exists() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            let result = store.save(&mut session).await.is_ok();
            disk::write(
                Path::new(&root).join(format!("result-{label}")),
                result.to_string(),
            )
            .unwrap();
        }
    }

    fn command(root: &Path, id: &str, mode: &str) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "session::store::process_tests::subprocess_probe",
                "--nocapture",
            ])
            .env("KYLER_ROLLOUT_TEST_ROOT", root)
            .env("KYLER_ROLLOUT_TEST_MODE", mode)
            .env("KYLER_ROLLOUT_TEST_ID", id)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
    async fn succeeded(child: tokio::process::Child) {
        let output =
            tokio::time::timeout(std::time::Duration::from_secs(10), child.wait_with_output())
                .await
                .unwrap()
                .unwrap();
        assert!(
            output.status.success(),
            "child failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn os_processes_cannot_recover_or_mutate_a_live_execution_or_share_a_writer() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).await.unwrap();
        let mut session = store.create().await.unwrap();
        session.begin_execution("running").unwrap();
        store.save(&mut session).await.unwrap();
        let local = acquire_session(&session.session_id).unwrap();
        let external = store.ensure_current(&session).await.unwrap();
        succeeded(
            command(dir.path(), &session.session_id, "active")
                .spawn()
                .unwrap(),
        )
        .await;
        drop(external);
        drop(local);
        let writer = RolloutWriter::open(
            dir.path().join(&session.session_id).join("rollout.jsonl"),
            &session.session_id,
        )
        .unwrap();
        succeeded(
            command(dir.path(), &session.session_id, "writer")
                .spawn()
                .unwrap(),
        )
        .await;
        drop(writer);
        assert_eq!(
            store
                .load(&session.session_id)
                .await
                .unwrap()
                .current_execution()
                .unwrap()
                .status,
            ExecutionStatus::Cancelled
        );
    }

    #[tokio::test]
    async fn os_processes_serialize_revision_checks_with_writes() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).await.unwrap();
        let session = store.create().await.unwrap();
        disk::write(
            dir.path().join("race-input.json"),
            serde_json::to_vec(&session).unwrap(),
        )
        .unwrap();
        let a = command(dir.path(), &session.session_id, "race")
            .env("KYLER_ROLLOUT_TEST_LABEL", "a")
            .spawn()
            .unwrap();
        let b = command(dir.path(), &session.session_id, "race")
            .env("KYLER_ROLLOUT_TEST_LABEL", "b")
            .spawn()
            .unwrap();
        disk::write(dir.path().join("start"), b"go").unwrap();
        tokio::join!(succeeded(a), succeeded(b));
        let a = disk::read_to_string(dir.path().join("result-a")).unwrap();
        let b = disk::read_to_string(dir.path().join("result-b")).unwrap();
        assert_ne!(a, b);
        assert_eq!(
            store.load(&session.session_id).await.unwrap().revision,
            session.revision + 1
        );
    }
}
