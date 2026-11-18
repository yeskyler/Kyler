//! Rebuildable session/run indexes and durable user preferences. Rollout stays authoritative.
use super::{journal::read_locked, validate_id};
use crate::{
    Session,
    context::{ContentItem, ExecutionStatus},
};

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use kyler_protocol::api::{
    run::{RunInfo, RunStatus},
    session::SessionSummary,
};

pub struct Catalog {
    connection: Mutex<Connection>,
    scope: String,
    pub path: PathBuf,
}
pub use kyler_protocol::api::preferences::{PreferenceUpdate, UiPreferences};

pub fn summary(session: &Session) -> SessionSummary {
    let title = session
        .events
        .iter()
        .flat_map(|event| &event.content)
        .find_map(|item| match item {
            ContentItem::Message { role, content } if role.as_str() == "user" => {
                let value = content.split_whitespace().collect::<Vec<_>>().join(" ");
                (!value.is_empty()).then(|| value.chars().take(80).collect())
            }
            _ => None,
        });
    SessionSummary {
        cwd: session
            .environment
            .as_ref()
            .map(|e| e.cwd.to_string_lossy().into_owned()),
        title,
        draft: false,
        project: session
            .environment
            .as_ref()
            .and_then(|e| e.project_root())
            .map(|path| path.to_string_lossy().into_owned()),
        session_id: session.session_id.clone(),
        created_at: session.created_at,
        updated_at: session.updated_at,
        event_count: session.events.len(),
        execution_count: session.executions.len() + usize::from(session.execution.is_some()),
        status: session.execution.as_ref().map(|e| match e.status {
            ExecutionStatus::Running => RunStatus::Running,
            ExecutionStatus::Completed => RunStatus::Completed,
            ExecutionStatus::Failed => RunStatus::Failed,
            ExecutionStatus::Cancelled => RunStatus::Cancelled,
        }),
    }
}
impl Catalog {
    pub fn open(root: &Path) -> Result<Self> {
        // The conventional sessions/ directory shares its configuration parent.
        // Arbitrary standalone stores keep their database within their own boundary.
        let parent = if root.file_name().is_some_and(|name| name == "sessions") {
            root.parent().unwrap_or(root)
        } else {
            root
        };
        let path = parent.join("app.sqlite3");
        let mut options = fs::OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            options.mode(0o600);
            options.open(&path)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(not(unix))]
        options.open(&path)?;
        let connection = Connection::open(&path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;PRAGMA foreign_keys=ON;PRAGMA synchronous=FULL;",
        )?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(
            version <= 3,
            "Application database schema is newer than this binary"
        );
        if version == 0 {
            connection.execute_batch(include_str!("catalog.sql"))?;
        } else if version < 3 {
            connection.execute_batch("BEGIN IMMEDIATE;")?;
            let current: i64 =
                connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
            if current == 1 {
                connection.execute_batch("ALTER TABLE sessions ADD COLUMN cwd TEXT;")?;
            }
            if current < 3 {
                connection.execute_batch("ALTER TABLE sessions ADD COLUMN project TEXT; ALTER TABLE sessions DROP COLUMN workspace_root; ALTER TABLE sessions DROP COLUMN workspace_mode; UPDATE sessions SET signature=''; PRAGMA user_version=3;")?;
            }
            connection.execute_batch("COMMIT;")?;
        }

        Ok(Self {
            connection: Mutex::new(connection),
            scope: root.to_string_lossy().into_owned(),
            path,
        })
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Application database unavailable"))
    }
    fn signature(root: &Path, id: &str) -> Result<String> {
        let log = root.join(id).join("rollout.jsonl");
        let path = if log.exists() {
            log
        } else {
            root.join(format!("{id}.json"))
        };
        let meta = fs::metadata(path)?;
        Ok(format!("{}:{:?}", meta.len(), meta.modified()?))
    }
    pub fn upsert(&self, root: &Path, session: &Session) -> Result<()> {
        let item = summary(session);
        let json = serde_json::to_string(&item)?;
        let preview = session
            .events
            .iter()
            .rev()
            .flat_map(|e| e.content.iter().rev())
            .find_map(|item| match item {
                ContentItem::Message { role, content }
                    if matches!(role.as_str(), "user" | "assistant") =>
                {
                    Some(content.chars().take(160).collect::<String>())
                }
                _ => None,
            });
        self.lock()?.execute("INSERT INTO sessions(scope,session_id,title,cwd,project,created_at,updated_at,revision,signature,summary_json,last_message_preview,is_draft) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(scope,session_id) DO UPDATE SET title=excluded.title,cwd=excluded.cwd,project=excluded.project,updated_at=excluded.updated_at,revision=excluded.revision,signature=excluded.signature,summary_json=excluded.summary_json,last_message_preview=excluded.last_message_preview,is_draft=excluded.is_draft",params![self.scope,session.session_id,item.title,item.cwd,item.project,item.created_at,item.updated_at,session.revision as i64,Self::signature(root,&session.session_id)?,json,preview,root.join("_runs").join(format!("{}.draft",session.session_id)).exists()])?;
        Ok(())
    }
    pub fn reconcile(&self, root: &Path) -> Result<()> {
        let mut ids = BTreeSet::new();
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            let candidate = if path.is_dir() {
                path.file_name()
            } else if path.extension().is_some_and(|e| e == "json") {
                path.file_stem()
            } else {
                None
            };
            if let Some(id) = candidate
                .and_then(|s| s.to_str())
                .filter(|id| validate_id(id).is_ok())
            {
                ids.insert(id.to_owned());
            }
        }
        for id in &ids {
            if !root.join(id).join("rollout.jsonl").exists()
                && !root.join(format!("{id}.json")).exists()
            {
                continue;
            }
            let signature = Self::signature(root, id)?;
            let saved: Option<String> = self
                .lock()?
                .query_row(
                    "SELECT signature FROM sessions WHERE scope=? AND session_id=?",
                    params![self.scope, id],
                    |row| row.get(0),
                )
                .optional()?;
            if saved.as_ref() != Some(&signature) {
                if let Some(loaded) = read_locked(root, id)? {
                    self.upsert(root, &loaded.session)?;
                }
            }
        }
        let known = self.ids()?;
        for id in known {
            if !ids.contains(&id) {
                self.remove(&id)?;
            }
        }
        // Draft markers are authoritative when recovering an interrupted publication.
        let connection = self.lock()?;
        for id in ids {
            connection.execute(
                "UPDATE sessions SET is_draft=? WHERE scope=? AND session_id=?",
                params![
                    root.join("_runs").join(format!("{id}.draft")).exists(),
                    self.scope,
                    id
                ],
            )?;
        }
        Ok(())
    }
    fn ids(&self) -> Result<Vec<String>> {
        let c = self.lock()?;
        Ok(c.prepare("SELECT session_id FROM sessions WHERE scope=?")?
            .query_map([&self.scope], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn list(&self, visible_only: bool) -> Result<Vec<SessionSummary>> {
        let c = self.lock()?;
        let values=c.prepare("SELECT summary_json FROM sessions WHERE scope=? AND (?=0 OR is_draft=0) ORDER BY updated_at DESC,session_id")?.query_map(params![self.scope,visible_only],|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        values
            .iter()
            .map(|value| Ok(serde_json::from_str(value)?))
            .collect()
    }
    pub fn page(
        &self,
        cursor: usize,
        limit: usize,
    ) -> Result<kyler_protocol::api::session::SessionPage> {
        ensure!((1..=500).contains(&limit), "Invalid page limit");
        let mut c = self.lock()?;
        let tx = c.transaction()?;
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM sessions WHERE scope=? AND is_draft=0",
            [&self.scope],
            |row| row.get(0),
        )?;
        ensure!(
            cursor <= count as usize,
            "Pagination cursor is out of range."
        );
        let rows=tx.prepare("SELECT summary_json FROM sessions WHERE scope=? AND is_draft=0 ORDER BY updated_at DESC,session_id LIMIT ? OFFSET ?")?.query_map(params![self.scope,limit as i64,cursor as i64],|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let sessions = rows
            .iter()
            .map(|row| serde_json::from_str(row))
            .collect::<serde_json::Result<Vec<_>>>()?;
        let end = cursor.saturating_add(limit).min(count as usize);
        tx.commit()?;
        Ok(kyler_protocol::api::session::SessionPage {
            sessions,
            next_cursor: (end < count as usize).then_some(end),
        })
    }
    pub fn draft(&self, id: &str, draft: bool) -> Result<()> {
        self.lock()?.execute(
            "UPDATE sessions SET is_draft=? WHERE scope=? AND session_id=?",
            params![draft, self.scope, id],
        )?;
        Ok(())
    }
    pub fn remove(&self, id: &str) -> Result<()> {
        let mut c = self.lock()?;
        let tx = c.transaction()?;
        tx.execute(
            "DELETE FROM sessions WHERE scope=? AND session_id=?",
            params![self.scope, id],
        )?;
        tx.execute(
            "DELETE FROM session_runs WHERE scope=? AND session_id=?",
            params![self.scope, id],
        )?;
        tx.execute("UPDATE ui_preferences SET last_session_id=NULL,revision=revision+1 WHERE scope=? AND last_session_id=?",params![self.scope,id])?;
        tx.commit()?;
        Ok(())
    }
    fn write_run(connection: &Connection, scope: &str, info: &RunInfo) -> Result<()> {
        connection.execute("INSERT INTO session_runs(scope,run_id,session_id,execution_id,status,last_committed_event_seq,started_at,finished_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(scope,run_id) DO UPDATE SET execution_id=excluded.execution_id,status=excluded.status,last_committed_event_seq=excluded.last_committed_event_seq,finished_at=excluded.finished_at",params![scope,info.run_id,info.session_id,info.execution_id,serde_json::to_value(info.status)?.as_str().expect("run status is a string").to_owned(),info.last_event_seq as i64,info.created_at,info.finished_at])?;
        Ok(())
    }
    pub fn run(&self, info: &RunInfo) -> Result<()> {
        Self::write_run(&*self.lock()?, &self.scope, info)
    }
    pub fn replace_runs(&self, runs: &[RunInfo]) -> Result<()> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM session_runs WHERE scope=?", [&self.scope])?;
        for run in runs {
            Self::write_run(&transaction, &self.scope, run)?;
        }
        transaction.commit()?;
        Ok(())
    }
    pub fn remove_run(&self, id: &str) -> Result<()> {
        self.lock()?.execute(
            "DELETE FROM session_runs WHERE scope=? AND run_id=?",
            params![self.scope, id],
        )?;
        Ok(())
    }
    pub fn preferences(&self) -> Result<UiPreferences> {
        Ok(self.lock()?.query_row("SELECT last_session_id,sidebar_collapsed,revision FROM ui_preferences WHERE scope=?",[&self.scope],|row|Ok(UiPreferences{last_session_id:row.get(0)?,sidebar_collapsed:row.get(1)?,revision:row.get::<_,i64>(2)? as u64})).optional()?.unwrap_or_default())
    }
    pub fn update_preferences(&self, update: PreferenceUpdate) -> Result<UiPreferences> {
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO ui_preferences(scope,sidebar_collapsed,revision) VALUES(?,0,0)",
            [&self.scope],
        )?;
        if let Some(id) = update.last_session_id {
            if let Some(id) = &id {
                validate_id(id)?;
                let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE scope=? AND session_id=? AND is_draft=0)",params![self.scope,id],|row|row.get(0))?;
                ensure!(valid, "Recent session is unavailable");
            }
            tx.execute(
                "UPDATE ui_preferences SET last_session_id=? WHERE scope=?",
                params![id, self.scope],
            )?;
        }
        if let Some(value) = update.sidebar_collapsed {
            tx.execute(
                "UPDATE ui_preferences SET sidebar_collapsed=? WHERE scope=?",
                params![value, self.scope],
            )?;
        }
        tx.execute(
            "UPDATE ui_preferences SET revision=revision+1 WHERE scope=?",
            [&self.scope],
        )?;
        tx.commit()?;
        drop(c);
        self.preferences()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionStore;
    #[tokio::test]
    async fn indexes_migrate_repair_and_preferences_survive_without_loading_history() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sessions");
        let store = SessionStore::new(&directory).await.unwrap();
        let mut session = store.create().await.unwrap();
        session.begin_execution("Review my project").unwrap();
        store.save(&mut session).await.unwrap();
        let catalog = store.catalog();
        let preferences = catalog
            .update_preferences(PreferenceUpdate {
                last_session_id: Some(Some(session.session_id.clone())),
                sidebar_collapsed: Some(true),
            })
            .unwrap();
        assert_eq!(preferences.revision, 1);
        catalog
            .update_preferences(PreferenceUpdate {
                sidebar_collapsed: Some(false),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            catalog.preferences().unwrap().last_session_id,
            Some(session.session_id.clone())
        );
        assert_eq!(
            catalog.page(0, 50).unwrap().sessions[0].title.as_deref(),
            Some("Review my project")
        );
        // A missing/corrupted acceleration snapshot is not needed by the indexed list.
        fs::write(
            directory.join(&session.session_id).join("snapshot.json"),
            b"invalid cache",
        )
        .unwrap();
        assert_eq!(catalog.page(0, 50).unwrap().sessions.len(), 1);
        // Simulate a committed journal whose index update was interrupted.
        catalog
            .lock()
            .unwrap()
            .execute("UPDATE sessions SET signature='stale',title='stale'", [])
            .unwrap();
        drop(catalog);
        drop(store);
        let store = SessionStore::new(&directory).await.unwrap();
        assert_eq!(
            store.summaries(false).await.unwrap()[0].title.as_deref(),
            Some("Review my project")
        );
        assert!(!store.catalog().preferences().unwrap().sidebar_collapsed);
        let mut restored = store.load(&session.session_id).await.unwrap();
        restored.recover_interrupted().unwrap();
        store.save(&mut restored).await.unwrap();
        store.delete(&session.session_id).await.unwrap();
        assert!(store.summaries(false).await.unwrap().is_empty());
        assert_eq!(store.catalog().preferences().unwrap().last_session_id, None);
    }
    #[tokio::test]
    async fn draft_filter_scope_and_nullable_patches_are_independent() {
        let root = tempfile::tempdir().unwrap();
        let a = SessionStore::new(root.path()).await.unwrap();
        let b = SessionStore::new(root.path().join("sessions"))
            .await
            .unwrap();
        let session = a.create().await.unwrap();
        a.catalog().draft(&session.session_id, true).unwrap();
        assert!(a.catalog().page(0, 50).unwrap().sessions.is_empty());
        assert!(b.list().await.unwrap().is_empty());
        assert!(
            a.catalog()
                .update_preferences(PreferenceUpdate {
                    last_session_id: Some(Some(session.session_id.clone())),
                    ..Default::default()
                })
                .is_err()
        );
        a.catalog().draft(&session.session_id, false).unwrap();
        a.catalog()
            .update_preferences(PreferenceUpdate {
                last_session_id: Some(Some(session.session_id)),
                ..Default::default()
            })
            .unwrap();
        let patch: PreferenceUpdate = serde_json::from_str("{\"last_session_id\":null}").unwrap();
        assert_eq!(patch.last_session_id, Some(None));
        a.catalog().update_preferences(patch).unwrap();
        assert_eq!(a.catalog().preferences().unwrap().last_session_id, None);
        assert!(a.catalog().page(99, 50).is_err());
    }
    #[tokio::test]
    async fn existing_legacy_sessions_are_imported_and_deleted_catalog_rows_rebuild() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sessions");
        fs::create_dir(&directory).unwrap();
        let session = Session::new();
        fs::write(
            directory.join(format!("{}.json", session.session_id)),
            serde_json::to_vec(&session).unwrap(),
        )
        .unwrap();
        let store = SessionStore::new(&directory).await.unwrap();
        assert_eq!(
            store.page(0, 50).await.unwrap().sessions[0].session_id,
            session.session_id
        );
        store.load(&session.session_id).await.unwrap();
        assert!(
            directory
                .join(&session.session_id)
                .join("rollout.jsonl")
                .exists()
        );
        let catalog = store.catalog();
        catalog
            .lock()
            .unwrap()
            .execute("DELETE FROM sessions WHERE scope=?", [&catalog.scope])
            .unwrap();
        drop(catalog);
        drop(store);
        let restored = SessionStore::new(&directory).await.unwrap();
        assert_eq!(restored.list().await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn old_indexes_drop_obsolete_columns_without_losing_preferences() {
        for version in [1, 2] {
            let root = tempfile::tempdir().unwrap();
            let directory = root.path().join("sessions");
            fs::create_dir(&directory).unwrap();
            let directory = directory.canonicalize().unwrap();
            let database = root.path().join("app.sqlite3");
            let connection = Connection::open(&database).unwrap();
            let old_schema = include_str!("catalog.sql")
                .replace(
                    "cwd TEXT, project TEXT,",
                    if version == 1 {
                        "workspace_root TEXT, workspace_mode TEXT,"
                    } else {
                        "cwd TEXT, workspace_root TEXT, workspace_mode TEXT,"
                    },
                )
                .replace("user_version=3", &format!("user_version={version}"));
            connection.execute_batch(&old_schema).unwrap();
            connection
                .execute(
                    "INSERT INTO ui_preferences(scope,sidebar_collapsed,revision) VALUES(?,1,7)",
                    [directory.to_string_lossy().as_ref()],
                )
                .unwrap();
            drop(connection);
            let mut session = Session::new();
            session.environment = Some(
                serde_json::from_value(serde_json::json!({
                    "cwd":root.path(),
                    "sandbox":{"base_directory":root.path(),"policy_id":"fixture"}
                }))
                .unwrap(),
            );
            fs::write(
                directory.join(format!("{}.json", session.session_id)),
                serde_json::to_vec(&session).unwrap(),
            )
            .unwrap();
            let store = SessionStore::new(&directory).await.unwrap();
            assert_eq!(
                store.page(0, 50).await.unwrap().sessions[0].cwd.as_deref(),
                root.path().to_str()
            );
            assert!(store.catalog().preferences().unwrap().sidebar_collapsed);
            assert_eq!(store.catalog().preferences().unwrap().revision, 7);
            let connection = Connection::open(database).unwrap();
            assert_eq!(
                connection
                    .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                3
            );
            connection
                .prepare("SELECT cwd,project FROM sessions")
                .unwrap();
            assert!(
                connection
                    .prepare("SELECT workspace_root FROM sessions")
                    .is_err()
            );
            assert!(
                connection
                    .prepare("SELECT workspace_mode FROM sessions")
                    .is_err()
            );
            drop(store);
            assert!(SessionStore::new(&directory).await.is_ok());
        }
    }
}
