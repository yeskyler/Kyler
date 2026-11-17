use crate::{
    Error, Store,
    executor::{ScheduledExecutor, StartOutcome},
    invalid, schedule,
    store::Lease,
};

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, ensure};
use tokio::{sync::watch, task::JoinHandle};

use kyler_protocol::api::{
    automations::{
        SaveTaskRequest, ScheduledRun, ScheduledRunStatus as Status, ScheduledTask, TaskConfig,
    },
    permissions::PermissionMode,
};

pub struct Scheduler {
    pub store: Store,
    executor: Arc<dyn ScheduledExecutor>,
    concurrency: usize,
    running: AtomicBool,
}
pub struct SchedulerHandle {
    stop: watch::Sender<bool>,
    join: Option<JoinHandle<Result<()>>>,
}
impl SchedulerHandle {
    pub async fn stop(mut self) -> Result<()> {
        self.stop.send_replace(true);
        if let Some(join) = self.join.take() {
            join.await??;
        }
        Ok(())
    }
}
impl Drop for SchedulerHandle {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
impl Scheduler {
    pub async fn open(
        path: &Path,
        binding: &str,
        executor: Arc<dyn ScheduledExecutor>,
        concurrency: usize,
    ) -> Result<Arc<Self>> {
        ensure!(
            concurrency > 0 && concurrency <= 32,
            "Invalid scheduler concurrency"
        );
        Ok(Arc::new(Self {
            store: Store::open(path, binding).await?,
            executor,
            concurrency,
            running: AtomicBool::new(false),
        }))
    }
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
    pub async fn save_task(
        &self,
        id: Option<&str>,
        request: SaveTaskRequest,
    ) -> Result<ScheduledTask> {
        if id.is_none() {
            ensure!(
                request
                    .request_id
                    .as_ref()
                    .is_some_and(|value| uuid::Uuid::parse_str(value)
                        .is_ok_and(|id| id.to_string() == *value)),
                invalid("A canonical task creation request ID is required.")
            );
        }
        let mut config = request.config;
        validate(&config)?;
        ensure!(
            config.permission != PermissionMode::FullAccess || request.confirm_full_access,
            invalid("Confirm full access for this task revision.")
        );
        self.executor.validate(&mut config).await?;
        self.store
            .save_task(
                id,
                request.expected_revision,
                config,
                request.enabled,
                now(),
                request.request_id,
            )
            .await
    }
    pub async fn start(self: &Arc<Self>) -> Result<SchedulerHandle> {
        ensure!(
            !self.running.swap(true, Ordering::SeqCst),
            "Scheduler already started"
        );
        let lease = match self
            .store
            .acquire(uuid::Uuid::new_v4().to_string(), now())
            .await
        {
            Ok(lease) => lease,
            Err(error) => {
                self.running.store(false, Ordering::SeqCst);
                return Err(error);
            }
        };
        let (stop, receiver) = watch::channel(false);
        let scheduler = self.clone();
        let join = tokio::spawn(async move {
            let result = scheduler.run(lease.clone(), receiver).await;
            if result.is_err() {
                tracing::error!(
                    "automation scheduler stopped after an internal error; no new runs will be dispatched"
                );
            }
            if result.is_err() {
                tracing::error!(
                    "Automation scheduler stopped after a storage or ownership failure"
                );
            }
            scheduler.running.store(false, Ordering::SeqCst);
            let _ = scheduler.store.release(lease).await;
            result
        });
        Ok(SchedulerHandle {
            stop,
            join: Some(join),
        })
    }
    async fn run(&self, lease: Lease, mut stop: watch::Receiver<bool>) -> Result<()> {
        let alive = Arc::new(AtomicBool::new(true));
        let heartbeat_store = self.store.clone();
        let heartbeat_lease = lease.clone();
        let heartbeat_alive = alive.clone();
        let mut heartbeat_stop = stop.clone();
        let heartbeat = tokio::spawn(async move {
            loop {
                tokio::select! { _=heartbeat_stop.changed()=>break, _=tokio::time::sleep(Duration::from_secs(5))=>{
                    if heartbeat_store.heartbeat(heartbeat_lease.clone(),now()).await.is_err() { heartbeat_alive.store(false,Ordering::SeqCst); break; }
                } }
            }
        });
        let result = async {
            self.store.due(lease.clone(), now(), true).await?;
            // Previously queued intent is safe to submit. Other states must be reconciled.
            for run in self.store.active().await? {
                if run.status != Status::Queued {
                    self.observe(run, &lease, true).await?;
                }
            }
            loop {
                if *stop.borrow() {
                    break;
                }
                ensure!(alive.load(Ordering::SeqCst), Error::LeaseLost);
                self.tick(&lease).await?;
                tokio::select! {
                    _ = stop.changed() => break,
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }
            // Keep queued intent for restart, and request cancellation of admitted work.
            for run in self.store.active().await? {
                if run.status != Status::Queued {
                    let _ =
                        tokio::time::timeout(Duration::from_secs(5), self.executor.cancel(&run))
                            .await;
                }
            }
            Ok(())
        }
        .await;
        heartbeat.abort();
        let _ = heartbeat.await;
        result
    }
    async fn tick(&self, lease: &Lease) -> Result<()> {
        self.store.heartbeat(lease.clone(), now()).await?;
        self.store.due(lease.clone(), now(), false).await?;
        for run in self.store.active().await? {
            if run.status != Status::Queued {
                self.observe(run, lease, false).await?;
            }
        }
        for mut run in self
            .store
            .claim(lease.clone(), now(), self.concurrency)
            .await?
        {
            // Fence immediately before crossing into the host's admission path.
            self.store.heartbeat(lease.clone(), now()).await?;
            let outcome =
                tokio::time::timeout(Duration::from_secs(10), self.executor.start(&run)).await;
            let mut pause = false;
            match outcome {
                Ok(StartOutcome::Started(reference)) => {
                    run.status = Status::Running;
                    run.core_run_id = Some(reference.run_id);
                    run.execution_id = reference.execution_id;
                    run.last_observed_at = now();
                }
                Ok(StartOutcome::Deferred) => {
                    run.status = Status::Queued;
                    run.started_at = None;
                }
                Ok(StartOutcome::Failed(message)) => {
                    run.status = Status::Failed;
                    run.error = Some(message);
                    run.finished_at = Some(now());
                }
                Ok(StartOutcome::Unknown(message)) => {
                    run.status = Status::Interrupted;
                    run.error = Some(message);
                    run.finished_at = Some(now());
                    pause = true;
                }
                Err(_) => {
                    run.status = Status::Interrupted;
                    run.error=Some("Admission timed out and its outcome is unknown. Task paused; inspect the linked session before enabling it.".into());
                    run.finished_at = Some(now());
                    pause = true;
                }
            }
            self.store
                .save_run(run, lease.clone(), now(), pause)
                .await?;
        }
        Ok(())
    }
    async fn observe(&self, mut run: ScheduledRun, lease: &Lease, recovering: bool) -> Result<()> {
        let observed =
            tokio::time::timeout(Duration::from_secs(10), self.executor.inspect(&run)).await;
        let state = match observed {
            Ok(Ok(Some(state))) => state,
            Ok(Ok(None)) => {
                run.status = Status::Interrupted;
                run.finished_at = Some(now());
                run.error=Some("Previous execution outcome cannot be established. Task paused; inspect the linked session before enabling it.".into());
                return self.store.save_run(run, lease.clone(), now(), true).await;
            }
            // Keep enforcing deadlines and cancellation against the last confirmed state
            // during transient inspection failures, without claiming completion.
            _ if !recovering => crate::executor::ExecutionState {
                status: run.status,
                reference: None,
                summary: None,
                error: None,
            },
            _ => {
                run.status = Status::Interrupted;
                run.finished_at = Some(now());
                run.error = Some(
                    "Recovery could not verify the previous execution. Task paused for inspection."
                        .into(),
                );
                return self.store.save_run(run, lease.clone(), now(), true).await;
            }
        };
        let time = now();
        if !recovering && matches!(run.status, Status::Running | Status::Starting) {
            run.active_elapsed_ms += time.saturating_sub(run.last_observed_at);
        }
        run.last_observed_at = time;
        if let Some(reference) = state.reference {
            run.core_run_id = Some(reference.run_id);
            if reference.execution_id.is_some() {
                run.execution_id = reference.execution_id;
            }
        }
        let waiting = matches!(
            state.status,
            Status::WaitingForApproval | Status::WaitingForInput
        );
        run.waiting_since = if waiting {
            run.waiting_since.or(Some(time))
        } else {
            None
        };
        run.status = state.status;
        if state.status.is_terminal() {
            run.finished_at = Some(time);
            run.summary = state.summary;
            if run.timed_out {
                run.status = Status::Failed;
            } else {
                run.error = state.error;
            }
        } else {
            if run.active_elapsed_ms >= i64::from(run.snapshot.timeout_seconds) * 1000 {
                run.timed_out = true;
                run.cancel_requested = true;
                run.error = Some("Execution time limit exceeded.".into());
            }
            if run.waiting_since.is_some_and(|start| {
                time.saturating_sub(start) >= i64::from(run.snapshot.wait_timeout_seconds) * 1000
            }) {
                run.timed_out = true;
                run.cancel_requested = true;
                run.error = Some("Approval or user-input wait time limit exceeded.".into());
            }
            if run.cancel_requested {
                let _ =
                    tokio::time::timeout(Duration::from_secs(5), self.executor.cancel(&run)).await;
            }
        }
        self.store.save_run(run, lease.clone(), time, false).await
    }
}
pub fn validate(config: &TaskConfig) -> Result<()> {
    ensure!(
        !config.name.trim().is_empty() && config.name.len() <= 100 && !config.name.contains('\0'),
        invalid("Task name must contain 1 to 100 bytes.")
    );
    ensure!(
        !config.prompt.trim().is_empty()
            && config.prompt.len() <= 32000
            && !config.prompt.contains('\0'),
        invalid("Instructions must contain 1 to 32000 bytes.")
    );
    ensure!(
        !config.workspace_root.is_empty()
            && config.workspace_root.len() <= 4096
            && !config.workspace_root.contains('\0'),
        invalid("Choose a workspace.")
    );
    ensure!(
        (1..=86400).contains(&config.timeout_seconds)
            && (1..=86400).contains(&config.wait_timeout_seconds),
        invalid("Timeouts must be between 1 and 86400 seconds.")
    );
    ensure!(
        config.permission != PermissionMode::AutoReview,
        invalid("Choose a supported permission preset.")
    );
    schedule::next(&config.cron, &config.timezone, now())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{ExecutionRef, ExecutionState};
    use std::sync::atomic::AtomicUsize;
    struct Fake {
        starts: AtomicUsize,
        cancels: AtomicUsize,
        unknown: bool,
        state: std::sync::Mutex<Status>,
    }
    #[async_trait::async_trait]
    impl ScheduledExecutor for Fake {
        async fn validate(&self, _: &mut TaskConfig) -> Result<()> {
            Ok(())
        }
        async fn start(&self, _: &ScheduledRun) -> StartOutcome {
            self.starts.fetch_add(1, Ordering::SeqCst);
            if self.unknown {
                StartOutcome::Unknown("Unknown admission".into())
            } else {
                StartOutcome::Started(ExecutionRef {
                    run_id: "core-run".into(),
                    execution_id: Some("execution".into()),
                })
            }
        }
        async fn inspect(&self, _: &ScheduledRun) -> Result<Option<ExecutionState>> {
            Ok(Some(ExecutionState {
                status: *self.state.lock().unwrap(),
                reference: None,
                summary: Some("Result".into()),
                error: None,
            }))
        }
        async fn cancel(&self, _: &ScheduledRun) -> Result<()> {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    #[tokio::test]
    async fn unknown_admission_pauses_without_replaying() {
        let root = tempfile::tempdir().unwrap();
        let executor = Arc::new(Fake {
            starts: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
            unknown: true,
            state: std::sync::Mutex::new(Status::Running),
        });
        let scheduler = Scheduler::open(
            &root.path().join("scheduler.sqlite3"),
            "sessions",
            executor.clone(),
            2,
        )
        .await
        .unwrap();
        let task = scheduler
            .store
            .save_task(None, None, crate::tests::config(), true, now(), None)
            .await
            .unwrap();
        scheduler
            .store
            .enqueue(&task.id, "request", now())
            .await
            .unwrap();
        let lease = scheduler
            .store
            .acquire("owner".into(), now())
            .await
            .unwrap();
        scheduler.tick(&lease).await.unwrap();
        scheduler.tick(&lease).await.unwrap();
        assert_eq!(executor.starts.load(Ordering::SeqCst), 1);
        let task = scheduler.store.task(&task.id).await.unwrap();
        assert!(!task.enabled);
        assert_eq!(task.latest_run.unwrap().status, Status::Interrupted);
    }
    #[tokio::test]
    async fn wait_timeout_cancels_and_completion_is_recorded() {
        let root = tempfile::tempdir().unwrap();
        let executor = Arc::new(Fake {
            starts: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
            unknown: false,
            state: std::sync::Mutex::new(Status::WaitingForInput),
        });
        let scheduler = Scheduler::open(
            &root.path().join("scheduler.sqlite3"),
            "sessions",
            executor.clone(),
            1,
        )
        .await
        .unwrap();
        let task = scheduler
            .store
            .save_task(None, None, crate::tests::config(), false, now(), None)
            .await
            .unwrap();
        scheduler
            .store
            .enqueue(&task.id, "request", now())
            .await
            .unwrap();
        let lease = scheduler
            .store
            .acquire("owner".into(), now())
            .await
            .unwrap();
        scheduler.tick(&lease).await.unwrap();
        let mut run = scheduler.store.active().await.unwrap().remove(0);
        run.waiting_since = Some(now() - 31000);
        run.active_elapsed_ms = 100;
        scheduler
            .store
            .save_run(run, lease.clone(), now(), false)
            .await
            .unwrap();
        scheduler.tick(&lease).await.unwrap();
        assert_eq!(executor.cancels.load(Ordering::SeqCst), 1);
        let run = scheduler.store.active().await.unwrap().remove(0);
        assert!(run.timed_out);
        assert!(run.active_elapsed_ms < 1000);
        *executor.state.lock().unwrap() = Status::Cancelled;
        scheduler.tick(&lease).await.unwrap();
        let run = scheduler
            .store
            .runs(&task.id, None, 20)
            .await
            .unwrap()
            .runs
            .remove(0);
        assert_eq!(run.status, Status::Failed);
        assert!(run.error.unwrap().contains("wait time"));
        assert_eq!(executor.starts.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn queued_intent_survives_restart_and_completed_runs_are_not_replayed() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("scheduler.sqlite3");
        let executor = Arc::new(Fake {
            starts: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
            unknown: false,
            state: std::sync::Mutex::new(Status::Completed),
        });
        let store = Store::open(&path, "sessions").await.unwrap();
        let task = store
            .save_task(None, None, crate::tests::config(), false, now(), None)
            .await
            .unwrap();
        let run = store.enqueue(&task.id, "request", now()).await.unwrap();
        drop(store);
        let scheduler = Scheduler::open(&path, "sessions", executor.clone(), 1)
            .await
            .unwrap();
        let handle = scheduler.start().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if scheduler.store.runs(&task.id, None, 20).await.unwrap().runs[0].status
                    == Status::Completed
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        handle.stop().await.unwrap();
        assert_eq!(
            scheduler.store.runs(&task.id, None, 20).await.unwrap().runs[0].session_id,
            run.session_id
        );
        let handle = scheduler.start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.stop().await.unwrap();
        assert_eq!(executor.starts.load(Ordering::SeqCst), 1);
    }
    struct Unavailable(AtomicUsize);
    #[async_trait::async_trait]
    impl ScheduledExecutor for Unavailable {
        async fn validate(&self, _: &mut TaskConfig) -> Result<()> {
            Ok(())
        }
        async fn start(&self, _: &ScheduledRun) -> StartOutcome {
            StartOutcome::Deferred
        }
        async fn inspect(&self, _: &ScheduledRun) -> Result<Option<ExecutionState>> {
            anyhow::bail!("Transient inspection failure")
        }
        async fn cancel(&self, _: &ScheduledRun) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    #[tokio::test]
    async fn transient_inspection_errors_do_not_disable_execution_deadlines() {
        let root = tempfile::tempdir().unwrap();
        let executor = Arc::new(Unavailable(AtomicUsize::new(0)));
        let scheduler = Scheduler::open(
            &root.path().join("scheduler.sqlite3"),
            "sessions",
            executor.clone(),
            1,
        )
        .await
        .unwrap();
        let mut config = crate::tests::config();
        config.timeout_seconds = 1;
        let task = scheduler
            .store
            .save_task(None, None, config, false, now(), None)
            .await
            .unwrap();
        scheduler
            .store
            .enqueue(&task.id, "request", now())
            .await
            .unwrap();
        let lease = scheduler
            .store
            .acquire("owner".into(), now())
            .await
            .unwrap();
        let mut run = scheduler
            .store
            .claim(lease.clone(), now(), 1)
            .await
            .unwrap()
            .remove(0);
        run.status = Status::Running;
        run.last_observed_at = now() - 2000;
        scheduler
            .store
            .save_run(run.clone(), lease.clone(), now(), false)
            .await
            .unwrap();
        scheduler.observe(run, &lease, false).await.unwrap();
        assert_eq!(executor.0.load(Ordering::SeqCst), 1);
        let run = scheduler.store.active().await.unwrap().remove(0);
        assert!(run.timed_out);
        assert!(run.cancel_requested);
        assert_eq!(run.status, Status::Running);
    }
}
