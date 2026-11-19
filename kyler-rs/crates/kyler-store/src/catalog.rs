//! Scoped application indexes and durable UI preferences.
use crate::Database;

use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

pub use kyler_protocol::api::preferences::{PreferenceUpdate, UiPreferences};
use kyler_protocol::api::{run::RunInfo, session::SessionSummary};

pub struct Catalog {
    database: Database,
    scope: String,
    pub path: PathBuf,
}

pub struct SessionRecord {
    pub summary: SessionSummary,
    pub revision: u64,
    pub signature: String,
    pub last_message_preview: Option<String>,
    pub draft: bool,
}

impl Catalog {
    pub fn open(path: &Path, scope: String) -> Result<Self> {
        let database = Database::open(path)?;
        database.migrate(3, |transaction, current| {
            if current == 0 {
                transaction.execute_batch(include_str!("catalog.sql"))?;
            } else {
                if current == 1 {
                    transaction.execute_batch("ALTER TABLE sessions ADD COLUMN cwd TEXT;")?;
                }
                transaction.execute_batch("ALTER TABLE sessions ADD COLUMN project TEXT; ALTER TABLE sessions DROP COLUMN workspace_root; ALTER TABLE sessions DROP COLUMN workspace_mode; UPDATE sessions SET signature='';")?;
            }
            Ok(())
        })?;
        Ok(Self {
            database,
            scope,
            path: path.to_owned(),
        })
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.database.lock()
    }
    pub fn upsert(&self, record: &SessionRecord) -> Result<()> {
        let item = &record.summary;
        let json = serde_json::to_string(item)?;
        self.lock()?.execute("INSERT INTO sessions(scope,session_id,title,cwd,project,created_at,updated_at,revision,signature,summary_json,last_message_preview,is_draft) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(scope,session_id) DO UPDATE SET title=excluded.title,cwd=excluded.cwd,project=excluded.project,updated_at=excluded.updated_at,revision=excluded.revision,signature=excluded.signature,summary_json=excluded.summary_json,last_message_preview=excluded.last_message_preview,is_draft=excluded.is_draft",params![self.scope,item.session_id,item.title,item.cwd,item.project,item.created_at,item.updated_at,record.revision as i64,record.signature,json,record.last_message_preview,record.draft])?;
        Ok(())
    }
    pub fn signature(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .lock()?
            .query_row(
                "SELECT signature FROM sessions WHERE scope=? AND session_id=?",
                params![self.scope, id],
                |row| row.get(0),
            )
            .optional()?)
    }
    pub fn ids(&self) -> Result<Vec<String>> {
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
                ensure!(
                    Uuid::parse_str(id)?.to_string() == *id,
                    "session ID must be a canonical UUID"
                );
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
