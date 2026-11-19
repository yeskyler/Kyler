//! Transactional persistence. Agent admission always happens after committing intent.
use crate::{Error, conflict, schedule};

use std::path::Path;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use kyler_store::Database;

use kyler_protocol::api::automations::{
    ScheduledRun, ScheduledRunPage, ScheduledRunStatus as Status, ScheduledTask, TaskConfig,
};

const ACTIVE: &str = "'queued','starting','running','waiting_for_approval','waiting_for_input'";
#[derive(Clone)]
pub struct Store {
    database: Database,
}
#[derive(Clone)]
pub(crate) struct Lease {
    pub owner: String,
    pub epoch: i64,
}
fn status(value: Status) -> String {
    serde_json::to_value(value)
        .expect("status serializes")
        .as_str()
        .unwrap()
        .to_owned()
}
fn task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledTask> {
    let config: String = row.get(2)?;
    Ok(ScheduledTask {
        id: row.get(0)?,
        revision: u64::try_from(row.get::<_, i64>(1)?)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(1, -1))?,
        config: serde_json::from_str(&config).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        enabled: row.get(3)?,
        deleted: row.get(4)?,
        next_run_at: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        latest_run: None,
        has_unfinished_run: false,
    })
}
fn read_run(connection: &Connection, id: &str) -> Result<ScheduledRun> {
    let json: Option<String> = connection
        .query_row(
            "SELECT run_json FROM scheduled_runs WHERE id=?",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    serde_json::from_str(&json.ok_or(Error::NotFound)?).context("Invalid scheduled run record")
}
fn read_task(connection: &Connection, id: &str) -> Result<ScheduledTask> {
    let mut task = connection.query_row("SELECT id,revision,config_json,enabled,deleted,next_run_at,created_at,updated_at FROM scheduled_tasks WHERE id=?", [id], task_row).optional()?.ok_or(Error::NotFound)?;
    let latest: Option<String> = connection.query_row("SELECT run_json FROM scheduled_runs WHERE task_id=? ORDER BY created_at DESC,id DESC LIMIT 1", [id], |row| row.get(0)).optional()?;
    task.has_unfinished_run = active_count(connection, id)? > 0;
    task.latest_run = latest
        .map(|value| serde_json::from_str(&value))
        .transpose()?;
    Ok(task)
}
fn active_count(connection: &Connection, task: &str) -> Result<i64> {
    Ok(connection.query_row(
        &format!("SELECT count(*) FROM scheduled_runs WHERE task_id=? AND status IN ({ACTIVE})"),
        [task],
        |row| row.get(0),
    )?)
}
fn persist_run(connection: &Connection, run: &ScheduledRun) -> Result<()> {
    connection.execute(
        "UPDATE scheduled_runs SET status=?,run_json=? WHERE id=?",
        params![status(run.status), serde_json::to_string(run)?, run.id],
    )?;
    Ok(())
}
fn insert_run(
    connection: &Connection,
    task: &ScheduledTask,
    trigger: &str,
    key: &str,
    scheduled_for: i64,
    now: i64,
    skipped: Option<&str>,
) -> Result<ScheduledRun> {
    let existing: Option<String> = connection
        .query_row(
            "SELECT run_json FROM scheduled_runs WHERE task_id=? AND trigger_key=?",
            params![task.id, key],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        return Ok(serde_json::from_str(&existing)?);
    }
    let run = ScheduledRun {
        id: uuid::Uuid::new_v4().to_string(),
        task_id: task.id.clone(),
        task_revision: task.revision,
        snapshot: task.config.clone(),
        trigger: trigger.into(),
        scheduled_for,
        status: if skipped.is_some() {
            Status::Skipped
        } else {
            Status::Queued
        },
        session_id: uuid::Uuid::new_v4().to_string(),
        core_run_id: None,
        execution_id: None,
        created_at: now,
        started_at: None,
        finished_at: skipped.map(|_| now),
        last_observed_at: now,
        active_elapsed_ms: 0,
        waiting_since: None,
        cancel_requested: false,
        timed_out: false,
        summary: None,
        error: skipped.map(str::to_owned),
    };
    connection.execute("INSERT INTO scheduled_runs(id,task_id,trigger_key,status,session_id,created_at,run_json) VALUES(?,?,?,?,?,?,?)", params![run.id, task.id, key, status(run.status), run.session_id, now, serde_json::to_string(&run)?])?;
    Ok(run)
}
fn verify(connection: &Connection, lease: &Lease, now: i64) -> Result<()> {
    let valid: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM scheduler_leases WHERE name='scheduler' AND owner=? AND epoch=? AND expires_at>?)", params![lease.owner, lease.epoch, now], |row| row.get(0))?;
    ensure!(valid, Error::LeaseLost);
    Ok(())
}
impl Store {
    pub async fn open(path: &Path, binding: &str) -> Result<Self> {
        let path = path.to_owned();
        let binding = binding.to_owned();
        let database = tokio::task::spawn_blocking(move || -> Result<Database> {
            let database = Database::open(&path)?;
            database.migrate(1, |transaction, _| {
                transaction.execute_batch(include_str!("migrations/001_init.sql"))?;
                Ok(())
            })?;
            database.with(|connection| {
                connection.execute(
                    "INSERT OR IGNORE INTO scheduler_meta(key,value) VALUES('session_store',?)",
                    [&binding],
                )?;
                let saved: String = connection.query_row(
                    "SELECT value FROM scheduler_meta WHERE key='session_store'",
                    [],
                    |row| row.get(0),
                )?;
                ensure!(
                    saved == binding,
                    "Scheduler database belongs to a different session store"
                );
                Ok(())
            })?;
            Ok(database)
        })
        .await??;
        Ok(Self { database })
    }
    async fn with<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.database.with_async(operation).await
    }
    pub async fn tasks(&self) -> Result<Vec<ScheduledTask>> {
        self.with(|connection| {
            let ids = connection.prepare("SELECT id FROM scheduled_tasks WHERE deleted=0 ORDER BY created_at DESC,id DESC LIMIT 256")?.query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            ids.into_iter().map(|id| read_task(connection, &id)).collect()
        }).await
    }
    pub async fn task(&self, id: &str) -> Result<ScheduledTask> {
        let id = id.to_owned();
        self.with(move |connection| read_task(connection, &id))
            .await
    }
    pub async fn save_task(
        &self,
        id: Option<&str>,
        expected: Option<u64>,
        config: TaskConfig,
        enabled: bool,
        now: i64,
        creation_id: Option<String>,
    ) -> Result<ScheduledTask> {
        let id = id.map(str::to_owned);
        self.with(move |connection| {
            let next = if enabled { Some(schedule::next(&config.cron, &config.timezone, now)?) } else { None };
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let id = match id {
                Some(id) => {
                    let task = read_task(&transaction, &id)?;
                    ensure!(!task.deleted, Error::NotFound);
                    ensure!(expected == Some(task.revision), conflict("Task changed. Refresh before saving."));
                    transaction.execute("UPDATE scheduled_tasks SET revision=revision+1,config_json=?,enabled=?,next_run_at=?,updated_at=? WHERE id=?", params![serde_json::to_string(&config)?, enabled, next, now, id])?;
                    id
                }
                None => {
                    let id = creation_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                    let creation = serde_json::to_string(&(&config,enabled))?;
                    let prior: Option<String> = transaction.query_row("SELECT creation_json FROM scheduled_tasks WHERE id=?", [&id], |row| row.get(0)).optional()?;
                    if let Some(prior) = prior { ensure!(prior == creation, conflict("Task creation request was already used with different settings. Refresh the task list.")); return read_task(&transaction,&id); }
                    let count: i64 = transaction.query_row("SELECT count(*) FROM scheduled_tasks WHERE deleted=0", [], |row| row.get(0))?;
                    ensure!(count < 256, conflict("The automation limit of 256 tasks has been reached."));
                    transaction.execute("INSERT INTO scheduled_tasks(id,revision,config_json,creation_json,enabled,next_run_at,created_at,updated_at) VALUES(?,1,?,?,?,?,?,?)", params![id, serde_json::to_string(&config)?, creation, enabled, next, now, now])?;
                    id
                }
            };
            let task = read_task(&transaction, &id)?; transaction.commit()?; Ok(task)
        }).await
    }
    pub async fn set_enabled(
        &self,
        id: &str,
        expected: u64,
        enabled: bool,
        now: i64,
    ) -> Result<ScheduledTask> {
        let task = self.task(id).await?;
        self.save_task(Some(id), Some(expected), task.config, enabled, now, None)
            .await
    }
    pub async fn delete(&self, id: &str, expected: u64, now: i64) -> Result<()> {
        let id = id.to_owned();
        self.with(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let task = read_task(&transaction, &id)?;
            ensure!(task.revision == expected && !task.deleted, conflict("Task changed. Refresh before deleting."));
            ensure!(active_count(&transaction, &id)? == 0, conflict("Stop the active run before deleting this task."));
            transaction.execute("UPDATE scheduled_tasks SET deleted=1,enabled=0,next_run_at=NULL,revision=revision+1,updated_at=? WHERE id=?", params![now,id])?;
            transaction.commit()?; Ok(())
        }).await
    }
    pub async fn enqueue(&self, task_id: &str, request_id: &str, now: i64) -> Result<ScheduledRun> {
        let task_id = task_id.to_owned();
        let key = format!("manual:{request_id}");
        self.with(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let prior: Option<String> = transaction
                .query_row(
                    "SELECT run_json FROM scheduled_runs WHERE task_id=? AND trigger_key=?",
                    params![task_id, key],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(prior) = prior {
                return Ok(serde_json::from_str(&prior)?);
            }
            let task = read_task(&transaction, &task_id)?;
            ensure!(!task.deleted, Error::NotFound);
            ensure!(
                active_count(&transaction, &task_id)? == 0,
                conflict("This task already has an unfinished run.")
            );
            let run = insert_run(&transaction, &task, "manual", &key, now, now, None)?;
            transaction.commit()?;
            Ok(run)
        })
        .await
    }
    pub async fn runs(
        &self,
        task_id: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<ScheduledRunPage> {
        ensure!(
            (1..=200).contains(&limit),
            crate::invalid("History limit must be between 1 and 200.")
        );
        let task_id = task_id.to_owned();
        let cursor = cursor.map(str::to_owned);
        self.with(move |connection| {
            read_task(connection, &task_id)?;
            let position = cursor.map(|id| -> Result<(i64,String)> { let run = read_run(connection,&id)?; ensure!(run.task_id == task_id, Error::NotFound); Ok((run.created_at,id)) }).transpose()?;
            let (time, id) = position.unwrap_or((i64::MAX, "~".into()));
            let json = connection.prepare("SELECT run_json FROM scheduled_runs WHERE task_id=? AND (created_at<? OR (created_at=? AND id<?)) ORDER BY created_at DESC,id DESC LIMIT ?")?.query_map(params![task_id,time,time,id,(limit+1) as i64], |row| row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            let mut runs = json.into_iter().map(|value| serde_json::from_str(&value)).collect::<serde_json::Result<Vec<ScheduledRun>>>()?;
            let more = runs.len()>limit; runs.truncate(limit);
            Ok(ScheduledRunPage { next_cursor: more.then(|| runs.last().unwrap().id.clone()), runs })
        }).await
    }
    pub async fn cancel(&self, id: &str, now: i64) -> Result<ScheduledRun> {
        let id = id.to_owned();
        self.with(move |connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut run = read_run(&transaction, &id)?;
            if !run.status.is_terminal() {
                run.cancel_requested = true;
                if run.status == Status::Queued {
                    run.status = Status::Cancelled;
                    run.finished_at = Some(now);
                }
                persist_run(&transaction, &run)?;
            }
            transaction.commit()?;
            Ok(run)
        })
        .await
    }
    pub(crate) async fn acquire(&self, owner: String, now: i64) -> Result<Lease> {
        self.with(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let old: Option<(String,i64,i64)> = transaction.query_row("SELECT owner,epoch,expires_at FROM scheduler_leases WHERE name='scheduler'", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
            ensure!(old.as_ref().is_none_or(|value| value.0 == owner || value.2 <= now), conflict("Another scheduler owns this database."));
            let epoch = old.map_or(1, |value| value.1+1);
            transaction.execute("INSERT INTO scheduler_leases(name,owner,epoch,expires_at) VALUES('scheduler',?,?,?) ON CONFLICT(name) DO UPDATE SET owner=excluded.owner,epoch=excluded.epoch,expires_at=excluded.expires_at", params![owner,epoch,now+30000])?;
            transaction.commit()?; Ok(Lease { owner,epoch })
        }).await
    }
    pub(crate) async fn heartbeat(&self, lease: Lease, now: i64) -> Result<()> {
        self.with(move |connection| { let changed = connection.execute("UPDATE scheduler_leases SET expires_at=? WHERE name='scheduler' AND owner=? AND epoch=? AND expires_at>?", params![now+30000,lease.owner,lease.epoch,now])?; ensure!(changed==1,Error::LeaseLost); Ok(()) }).await
    }
    pub(crate) async fn release(&self, lease: Lease) -> Result<()> {
        self.with(move |connection| { connection.execute("UPDATE scheduler_leases SET expires_at=0 WHERE name='scheduler' AND owner=? AND epoch=?", params![lease.owner,lease.epoch])?; Ok(()) }).await
    }
    pub(crate) async fn active(&self) -> Result<Vec<ScheduledRun>> {
        self.with(|connection| connection.prepare(&format!("SELECT run_json FROM scheduled_runs WHERE status IN ({ACTIVE}) ORDER BY created_at,id"))?.query_map([], |row| row.get::<_,String>(0))?.map(|row| Ok(serde_json::from_str(&row?)?)).collect()).await
    }
    pub(crate) async fn save_run(
        &self,
        mut run: ScheduledRun,
        lease: Lease,
        now: i64,
        pause: bool,
    ) -> Result<()> {
        self.with(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            verify(&transaction,&lease,now)?;
            let current = read_run(&transaction,&run.id)?;
            if current.status.is_terminal() { return Ok(()); }
            run.cancel_requested |= current.cancel_requested;
            if run.cancel_requested && run.status == Status::Queued { run.status=Status::Cancelled; run.finished_at=Some(now); }
            if current.timed_out { run.timed_out=true; run.error=current.error; }
            persist_run(&transaction,&run)?;
            if pause { transaction.execute("UPDATE scheduled_tasks SET enabled=0,next_run_at=NULL,revision=revision+1,updated_at=? WHERE id=?", params![now,run.task_id])?; }
            transaction.commit()?; Ok(())
        }).await
    }
    pub(crate) async fn due(&self, lease: Lease, now: i64, recovering: bool) -> Result<()> {
        self.with(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            verify(&transaction,&lease,now)?;
            let tasks = transaction.prepare("SELECT id,revision,config_json,enabled,deleted,next_run_at,created_at,updated_at FROM scheduled_tasks WHERE enabled=1 AND deleted=0 AND next_run_at<=? ORDER BY next_run_at LIMIT 256")?.query_map([now], task_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            for task in tasks {
                let scheduled_for=task.next_run_at.unwrap();
                let missed = recovering || now.saturating_sub(scheduled_for)>60000;
                let reason = if missed { Some("Scheduled time was missed; automatic catch-up is disabled.") } else if active_count(&transaction,&task.id)?>0 { Some("Previous run is unfinished; overlapping execution was skipped.") } else { None };
                insert_run(&transaction,&task,"schedule",&format!("schedule:{scheduled_for}"),scheduled_for,now,reason)?;
                match schedule::next(&task.config.cron,&task.config.timezone,now) {
                    Ok(next) => { transaction.execute("UPDATE scheduled_tasks SET next_run_at=? WHERE id=?", params![next,task.id])?; }
                    Err(_) => { transaction.execute("UPDATE scheduled_tasks SET enabled=0,next_run_at=NULL,revision=revision+1 WHERE id=?", [&task.id])?; }
                }
            }
            transaction.commit()?; Ok(())
        }).await
    }
    pub(crate) async fn claim(
        &self,
        lease: Lease,
        now: i64,
        limit: usize,
    ) -> Result<Vec<ScheduledRun>> {
        self.with(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            verify(&transaction,&lease,now)?;
            let running: i64 = transaction.query_row(&format!("SELECT count(*) FROM scheduled_runs WHERE status IN ({ACTIVE}) AND status!='queued'"), [], |row| row.get(0))?;
            let free = limit.saturating_sub(running as usize);
            let ids = transaction.prepare("SELECT id FROM scheduled_runs WHERE status='queued' ORDER BY created_at,id LIMIT ?")?.query_map([free as i64], |row| row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            let mut runs=Vec::new();
            for id in ids { let mut run=read_run(&transaction,&id)?; run.status=Status::Starting; run.started_at=Some(now); run.last_observed_at=now; persist_run(&transaction,&run)?; runs.push(run); }
            transaction.commit()?; Ok(runs)
        }).await
    }
}
