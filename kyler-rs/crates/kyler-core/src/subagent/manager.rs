use super::{
    DelegateTaskArgs, ParentSessionRef, SubagentConfig, SubagentExecutor, SubagentStatus,
    SubagentTask, TaskOwner, TaskWaitResult, WaitMode,
};
use crate::{
    context::{ExecutionStatus, TokenUsage},
    session::{Session, SessionStore},
};

use std::{
    collections::{HashMap, HashSet},
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, ensure};
use futures::FutureExt;
use tokio::{
    runtime::Handle,
    sync::{Notify, OwnedSemaphorePermit, Semaphore, watch},
    task::JoinHandle,
};
use uuid::Uuid;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
struct TaskEntry {
    snapshot: Mutex<SubagentTask>,
    cancel: watch::Sender<bool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
struct Inner {
    config: SubagentConfig,
    tasks: Mutex<HashMap<String, Arc<TaskEntry>>>,
    slots: Arc<Semaphore>,
    notify: Notify,
    closed: AtomicBool,
}
#[derive(Clone)]
pub struct SubagentManager {
    inner: Arc<Inner>,
    store: Option<Arc<SessionStore>>,
}
impl SubagentManager {
    pub fn new(config: SubagentConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            inner: Arc::new(Inner {
                slots: Arc::new(Semaphore::new(config.max_concurrent_tasks)),
                config,
                tasks: Mutex::new(HashMap::new()),
                notify: Notify::new(),
                closed: AtomicBool::new(false),
            }),
            store: None,
        })
    }
    /// Child sessions are transient by default; attaching a store enables checkpoints.
    pub fn with_session_store(mut self, store: Arc<SessionStore>) -> Self {
        self.store = Some(store);
        self
    }
    pub(crate) fn child_store(&self) -> Option<Arc<SessionStore>> {
        self.store.clone()
    }
    pub fn config(&self) -> &SubagentConfig {
        &self.inner.config
    }
    pub fn open_scope(
        self: &Arc<Self>,
        owner: TaskOwner,
        executor: Arc<dyn SubagentExecutor>,
    ) -> Result<SubagentScope> {
        ensure!(
            !self.inner.closed.load(Ordering::SeqCst),
            "subagent manager is shut down"
        );
        Ok(SubagentScope {
            manager: self.clone(),
            owner,
            executor,
            submitted: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            entries: Mutex::new(Vec::new()),
        })
    }
    fn prune(&self, tasks: &mut HashMap<String, Arc<TaskEntry>>) {
        let cutoff = now().saturating_sub((self.inner.config.retention_secs * 1000) as i64);
        tasks.retain(|_, entry| {
            Arc::strong_count(entry) > 1
                || !lock(&entry.snapshot)
                    .finished_at
                    .is_some_and(|t| t <= cutoff)
        });
    }
    pub fn list_session(&self, session: &str) -> Vec<SubagentTask> {
        lock(&self.inner.tasks)
            .values()
            .filter_map(|entry| {
                let snapshot = lock(&entry.snapshot);
                (snapshot.parent_session_id == session).then(|| snapshot.clone())
            })
            .collect()
    }
    pub fn request_cancel(&self, session: &str, id: &str) -> Result<()> {
        let tasks = lock(&self.inner.tasks);
        let entry = tasks
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Task unavailable"))?;
        let snapshot = lock(&entry.snapshot);
        ensure!(snapshot.parent_session_id == session, "Task unavailable");
        if !snapshot.status.is_terminal() {
            let _ = entry.cancel.send(true);
        }
        Ok(())
    }
    pub fn cleanup(&self) {
        self.prune(&mut lock(&self.inner.tasks));
    }
    fn entry(&self, owner: &TaskOwner, id: &str) -> Result<Arc<TaskEntry>> {
        let mut tasks = lock(&self.inner.tasks);
        self.prune(&mut tasks);
        let entry = tasks
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("task not found in current parent execution"))?
            .clone();
        let snapshot = lock(&entry.snapshot);
        ensure!(
            snapshot.parent_session_id == owner.session_id
                && snapshot.parent_execution_id == owner.execution_id,
            "task not found in current parent execution"
        );
        drop(snapshot);
        Ok(entry)
    }
    fn list_owner(&self, owner: &TaskOwner) -> Vec<SubagentTask> {
        let mut tasks = lock(&self.inner.tasks);
        self.prune(&mut tasks);
        let mut result: Vec<_> = tasks
            .values()
            .filter_map(|e| {
                let s = lock(&e.snapshot);
                (s.parent_session_id == owner.session_id
                    && s.parent_execution_id == owner.execution_id)
                    .then(|| s.clone())
            })
            .collect();
        result.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.task_id.cmp(&b.task_id))
        });
        result
    }
    fn cancel_owner(&self, owner: &TaskOwner) -> Vec<Arc<TaskEntry>> {
        let tasks = lock(&self.inner.tasks);
        tasks
            .values()
            .filter_map(|entry| {
                let snapshot = lock(&entry.snapshot);
                if snapshot.parent_session_id != owner.session_id
                    || snapshot.parent_execution_id != owner.execution_id
                {
                    return None;
                }
                if !snapshot.status.is_terminal() {
                    let _ = entry.cancel.send(true);
                }
                Some(entry.clone())
            })
            .collect()
    }
    pub(crate) async fn finish_owner(&self, owner: &TaskOwner) -> Vec<SubagentTask> {
        let entries = self.cancel_owner(owner);
        self.wait_entries(&entries, WaitMode::All, None).await.tasks
    }
    async fn wait_entries(
        &self,
        entries: &[Arc<TaskEntry>],
        mode: WaitMode,
        timeout: Option<Duration>,
    ) -> TaskWaitResult {
        let wait = async {
            loop {
                // Register before reading state so a simultaneous completion cannot be lost.
                let changed = self.inner.notify.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let tasks: Vec<_> = entries.iter().map(|e| lock(&e.snapshot).clone()).collect();
                let ready = match mode {
                    WaitMode::Any => tasks.iter().any(|t| t.status.is_terminal()),
                    WaitMode::All => tasks.iter().all(|t| t.status.is_terminal()),
                };
                if ready {
                    return tasks;
                }
                changed.await;
            }
        };
        match timeout {
            Some(duration) => match tokio::time::timeout(duration, wait).await {
                Ok(tasks) => TaskWaitResult {
                    tasks,
                    timed_out: false,
                },
                Err(_) => TaskWaitResult {
                    tasks: entries.iter().map(|e| lock(&e.snapshot).clone()).collect(),
                    timed_out: true,
                },
            },
            None => TaskWaitResult {
                tasks: wait.await,
                timed_out: false,
            },
        }
    }
    pub async fn shutdown(&self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        let entries: Vec<_> = lock(&self.inner.tasks).values().cloned().collect();
        for entry in &entries {
            if !lock(&entry.snapshot).status.is_terminal() {
                let _ = entry.cancel.send(true);
            }
        }
        self.wait_entries(&entries, WaitMode::All, None).await;
    }
    async fn work(
        self: Arc<Self>,
        entry: Arc<TaskEntry>,
        mut session: Session,
        input: String,
        executor: Arc<dyn SubagentExecutor>,
        mut cancel: watch::Receiver<bool>,
        permit: Option<OwnedSemaphorePermit>,
    ) {
        let acquired = match permit {
            Some(permit) => Ok(permit),
            None => tokio::select! {biased;
                _=cancelled(&mut cancel)=>Err(SubagentStatus::Cancelled),
                result=tokio::time::timeout(Duration::from_secs(self.inner.config.queue_timeout_secs),self.inner.slots.clone().acquire_owned())=>match result {Ok(Ok(p))=>Ok(p),_=>Err(SubagentStatus::TimedOut)},
            },
        };
        let (status, output, review, mut error) = match acquired {
            Err(status) => (
                status,
                None,
                None,
                Some(
                    if status == SubagentStatus::Cancelled {
                        "Cancelled while queued"
                    } else {
                        "Subagent queue timeout"
                    }
                    .to_string(),
                ),
            ),
            Ok(_permit) => {
                {
                    let mut s = lock(&entry.snapshot);
                    s.status = SubagentStatus::Running;
                    s.started_at = Some(now());
                }
                self.inner.notify.notify_waiters();
                tokio::select! {biased;
                    _=cancelled(&mut cancel)=>(SubagentStatus::Cancelled,None,None,Some("Subagent cancelled; in-flight external tool outcomes may be unknown".into())),
                    result=async {
                        let session_id = session.session_id.clone();
                        let execution = AssertUnwindSafe(executor.execute_result(&mut session,&input)).catch_unwind();
                        tokio::pin!(execution);
                        let mut timer = tokio::time::interval(Duration::from_millis(50));
                        let mut previous = tokio::time::Instant::now(); let mut elapsed = Duration::ZERO;
                        loop { tokio::select! {
                            result = &mut execution => break Some(result),
                            _ = timer.tick() => {
                                let now = tokio::time::Instant::now();
                                if !executor.waiting_for_approval(&session_id) { elapsed += now - previous; }
                                previous = now;
                                if elapsed >= Duration::from_secs(self.inner.config.task_timeout_secs) { break None; }
                            }
                        }}
                    }=>match result {
                        Some(Ok(Ok(result)))=>(SubagentStatus::Completed,Some(result.output),result.review,None),
                        Some(Ok(Err(error)))=>(SubagentStatus::Failed,None,None,Some(format!("{error:#}"))),
                        Some(Err(_))=>(SubagentStatus::Failed,None,None,Some("Subagent panicked; external tool outcomes may be unknown".into())),
                        None=>(SubagentStatus::TimedOut,None,None,Some("Subagent execution timeout; external tool outcomes may be unknown".into())),
                    },
                }
            }
        };
        let review_task = lock(&entry.snapshot).agent == super::AgentKind::Review;
        if review_task && status != SubagentStatus::Completed {
            session.state.remove("kyler.review.result");
        }
        if session.is_running()
            || (review_task && status != SubagentStatus::Completed && session.execution.is_some())
        {
            let execution_status = if status == SubagentStatus::Completed {
                ExecutionStatus::Completed
            } else if status == SubagentStatus::Cancelled {
                ExecutionStatus::Cancelled
            } else {
                ExecutionStatus::Failed
            };
            if let Err(e) = session.finish(execution_status, output.clone(), error.clone()) {
                error = Some(format!("{}; {e:#}", error.unwrap_or_default()));
            }
        }
        if let Some(store) = &self.store {
            if let Err(e) = store.save(&mut session).await {
                error = Some(format!(
                    "{}; child session checkpoint failed: {e:#}",
                    error.unwrap_or_default()
                ));
            }
        }
        {
            let mut snapshot = lock(&entry.snapshot);
            // The worker owns the only terminal transition; cancellation only signals.
            snapshot.status = if status == SubagentStatus::Completed && error.is_some() {
                SubagentStatus::Failed
            } else {
                status
            };
            snapshot.output = output;
            snapshot.review = review;
            snapshot.error = error;
            snapshot.usage = session.usage;
            snapshot.execution_id = session.execution.as_ref().map(|e| e.execution_id.clone());
            snapshot.finished_at = Some(now());
        }
        lock(&entry.worker).take();
        self.inner.notify.notify_waiters();
    }
}
async fn cancelled(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

pub struct SubagentScope {
    manager: Arc<SubagentManager>,
    owner: TaskOwner,
    executor: Arc<dyn SubagentExecutor>,
    entries: Mutex<Vec<Arc<TaskEntry>>>,
    submitted: AtomicUsize,
    closed: AtomicBool,
}
impl SubagentScope {
    pub fn delegate(&self, request: DelegateTaskArgs, tool_call_id: &str) -> Result<SubagentTask> {
        request.validate()?;
        let executor = self
            .executor
            .for_task(&request)?
            .unwrap_or_else(|| self.executor.clone());
        let runtime = Handle::try_current()?;
        let mut tasks = lock(&self.manager.inner.tasks);
        ensure!(
            !self.closed.load(Ordering::SeqCst)
                && !self.manager.inner.closed.load(Ordering::SeqCst),
            "parent execution or subagent manager is closed"
        );
        self.manager.prune(&mut tasks);
        ensure!(
            self.submitted.load(Ordering::SeqCst) < self.manager.config().max_tasks_per_execution,
            "parent execution delegation limit reached"
        );
        if tasks.len() >= self.manager.config().max_retained_tasks {
            let oldest = tasks
                .iter()
                .filter(|(_, e)| Arc::strong_count(e) == 1)
                .filter_map(|(id, e)| lock(&e.snapshot).finished_at.map(|t| (id.clone(), t)))
                .min_by_key(|(_, t)| *t);
            if let Some((id, _)) = oldest {
                tasks.remove(&id);
            }
        }
        ensure!(
            tasks.len() < self.manager.config().max_retained_tasks,
            "subagent task capacity reached"
        );
        let permit = self.manager.inner.slots.clone().try_acquire_owned().ok();
        let queued = tasks
            .values()
            .filter(|e| lock(&e.snapshot).status == SubagentStatus::Queued)
            .count();
        ensure!(
            permit.is_some() || queued < self.manager.config().max_queued_tasks,
            "subagent queue is full"
        );
        let task_id = Uuid::new_v4().to_string();
        let mut session = Session::new();
        session.author.name =
            kyler_protocol::agents::subagent_name(self.submitted.load(Ordering::SeqCst));
        session.environment = executor.session_environment();
        session.parent = Some(ParentSessionRef {
            session_id: self.owner.session_id.clone(),
            execution_id: self.owner.execution_id.clone(),
            tool_call_id: tool_call_id.into(),
            task_id: task_id.clone(),
        });
        let snapshot = SubagentTask {
            author: session.author.clone(),
            agent: request.agent,
            review: None,
            task_id: task_id.clone(),
            parent_session_id: self.owner.session_id.clone(),
            parent_execution_id: self.owner.execution_id.clone(),
            parent_tool_call_id: tool_call_id.into(),
            session_id: session.session_id.clone(),
            execution_id: None,
            status: if permit.is_some() {
                SubagentStatus::Running
            } else {
                SubagentStatus::Queued
            },
            output: None,
            error: None,
            usage: TokenUsage::default(),
            created_at: now(),
            started_at: None,
            finished_at: None,
        };
        let (cancel, rx) = watch::channel(false);
        let entry = Arc::new(TaskEntry {
            snapshot: Mutex::new(snapshot.clone()),
            cancel,
            worker: Mutex::new(None),
        });
        tasks.insert(task_id, entry.clone());
        lock(&self.entries).push(entry.clone());
        self.submitted.fetch_add(1, Ordering::SeqCst);
        let input = serde_json::json!({"task":request.task,"context":request.context}).to_string();
        let manager = self.manager.clone();
        let worker_entry = entry.clone();
        // Spawn without waiting for a slot; queueing is owned by the worker.
        let worker = runtime.spawn(async move {
            manager
                .work(worker_entry, session, input, executor, rx, permit)
                .await;
        });
        *lock(&entry.worker) = Some(worker);
        Ok(snapshot)
    }
    pub fn get(&self, id: &str) -> Result<SubagentTask> {
        Ok(lock(&self.manager.entry(&self.owner, id)?.snapshot).clone())
    }
    pub fn list(&self) -> Vec<SubagentTask> {
        self.manager.list_owner(&self.owner)
    }
    pub async fn wait(
        &self,
        ids: &[String],
        mode: WaitMode,
        timeout_secs: u64,
    ) -> Result<TaskWaitResult> {
        ensure!(!ids.is_empty(), "task_ids cannot be empty");
        ensure!(
            ids.len() <= self.manager.config().max_tasks_per_execution,
            "too many task IDs"
        );
        ensure!(
            ids.iter().collect::<HashSet<_>>().len() == ids.len(),
            "duplicate task IDs"
        );
        let entries = ids
            .iter()
            .map(|id| self.manager.entry(&self.owner, id))
            .collect::<Result<Vec<_>>>()?;
        Ok(self
            .manager
            .wait_entries(
                &entries,
                mode,
                Some(Duration::from_secs(
                    timeout_secs.min(self.manager.config().max_wait_secs),
                )),
            )
            .await)
    }
    pub async fn cancel(&self, id: &str) -> Result<SubagentTask> {
        let entry = self.manager.entry(&self.owner, id)?;
        if !lock(&entry.snapshot).status.is_terminal() {
            let _ = entry.cancel.send(true);
        }
        self.manager
            .wait_entries(&[entry.clone()], WaitMode::All, None)
            .await;
        let snapshot = lock(&entry.snapshot).clone();
        Ok(snapshot)
    }
    pub async fn close(&self) -> Vec<SubagentTask> {
        self.closed.store(true, Ordering::SeqCst);
        self.manager.finish_owner(&self.owner).await
    }
}
impl Drop for SubagentScope {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        self.manager.cancel_owner(&self.owner);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::{
            DelegateTaskArgs, SubagentConfig, SubagentExecutor, SubagentStatus, TaskOwner, WaitMode,
        },
        SubagentManager,
    };
    use crate::{
        context::ExecutionStatus,
        session::{Session, SessionStore},
    };

    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use anyhow::Result;
    use tokio::sync::Semaphore;
    use uuid::Uuid;

    struct Gate {
        started: Semaphore,
        release: Semaphore,
        active: Arc<AtomicUsize>,
        peak: AtomicUsize,
    }
    impl Gate {
        fn new() -> Self {
            Self {
                started: Semaphore::new(0),
                release: Semaphore::new(0),
                active: Arc::new(AtomicUsize::new(0)),
                peak: AtomicUsize::new(0),
            }
        }
    }
    struct Active(Arc<AtomicUsize>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    #[async_trait::async_trait]
    impl SubagentExecutor for Gate {
        async fn execute(&self, session: &mut Session, input: &str) -> Result<String> {
            assert!(session.state.is_empty());
            assert!(session.parent.is_some());
            session.begin_execution(input)?;
            session.add_usage(10, 2, 12);
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            let _active = Active(self.active.clone());
            self.started.add_permits(1);
            self.release.acquire().await?.forget();
            let task: serde_json::Value = serde_json::from_str(input)?;
            if task["task"] == "fail" {
                anyhow::bail!("fixture failure");
            }
            if task["task"] == "panic" {
                panic!("fixture panic");
            }
            let output = format!("{}:{}", task["task"], session.session_id);
            session.finish(ExecutionStatus::Completed, Some(output.clone()), None)?;
            Ok(output)
        }
    }
    fn owner() -> TaskOwner {
        TaskOwner {
            session_id: Uuid::new_v4().to_string(),
            execution_id: Uuid::new_v4().to_string(),
        }
    }
    fn request(task: &str) -> DelegateTaskArgs {
        DelegateTaskArgs {
            agent: Default::default(),
            review: None,
            task: task.into(),
            context: Some("background".into()),
        }
    }
    async fn started(gate: &Gate, n: u32) {
        tokio::time::timeout(Duration::from_secs(2), gate.started.acquire_many(n))
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
    #[tokio::test]
    async fn delegation_returns_before_work_and_two_children_run_in_parallel() {
        let manager = Arc::new(
            SubagentManager::new(SubagentConfig {
                max_concurrent_tasks: 2,
                ..Default::default()
            })
            .unwrap(),
        );
        let gate = Arc::new(Gate::new());
        let scope = manager.open_scope(owner(), gate.clone()).unwrap();
        let first = scope.delegate(request("first"), "call1").unwrap();
        let second = scope.delegate(request("second"), "call2").unwrap();
        assert_eq!(first.author.name, "atlas");
        assert_eq!(second.author.name, "nova");
        assert_ne!(first.task_id, second.task_id);
        assert_ne!(first.session_id, second.session_id);
        started(&gate, 2).await;
        assert_eq!(gate.peak.load(Ordering::SeqCst), 2);
        let ids = vec![first.task_id.clone(), second.task_id.clone()];
        let pending = scope.wait(&ids, WaitMode::All, 0).await.unwrap();
        assert!(pending.timed_out);
        assert!(
            pending
                .tasks
                .iter()
                .all(|t| t.status == SubagentStatus::Running)
        );
        gate.release.add_permits(1);
        let any = scope.wait(&ids, WaitMode::Any, 2).await.unwrap();
        assert!(!any.timed_out);
        assert_eq!(
            any.tasks
                .iter()
                .filter(|t| t.status == SubagentStatus::Completed)
                .count(),
            1
        );
        gate.release.add_permits(1);
        let all = scope.wait(&ids, WaitMode::All, 2).await.unwrap();
        assert!(!all.timed_out);
        assert!(
            all.tasks
                .iter()
                .all(|t| t.status == SubagentStatus::Completed && t.usage.total_tokens == 12)
        );
        let mut parent = Session::new();
        parent.begin_execution("parent").unwrap();
        // This test scope belongs to a separate owner; align the bookkeeping fixture.
        let mut tasks = all.tasks;
        for task in &mut tasks {
            task.parent_session_id = parent.session_id.clone();
            task.parent_execution_id = parent.current_execution().unwrap().execution_id.clone();
        }
        parent.record_subagent_tasks(&tasks);
        parent.record_subagent_tasks(&tasks);
        assert_eq!(parent.subagent_usage.total_tokens, 24);
        assert_eq!(parent.usage.total_tokens, 0);
        assert_eq!(
            parent
                .current_execution()
                .unwrap()
                .subagent_usage
                .total_tokens,
            24
        );
        assert_eq!(
            scope.cancel(&first.task_id).await.unwrap().status,
            SubagentStatus::Completed
        );
    }
    #[tokio::test]
    async fn queue_limits_cancel_and_parent_scope_isolation() {
        let manager = Arc::new(
            SubagentManager::new(SubagentConfig {
                max_concurrent_tasks: 1,
                max_queued_tasks: 1,
                max_tasks_per_execution: 3,
                ..Default::default()
            })
            .unwrap(),
        );
        let gate = Arc::new(Gate::new());
        let scope = manager.open_scope(owner(), gate.clone()).unwrap();
        let a = scope.delegate(request("a"), "a").unwrap();
        started(&gate, 1).await;
        let b = scope.delegate(request("b"), "b").unwrap();
        assert_eq!(b.status, SubagentStatus::Queued);
        assert!(scope.delegate(request("full"), "c").is_err());
        let other = manager.open_scope(owner(), gate.clone()).unwrap();
        assert!(other.get(&a.task_id).is_err());
        assert!(other.cancel(&a.task_id).await.is_err());
        assert!(
            other
                .wait(&[a.task_id.clone()], WaitMode::Any, 0)
                .await
                .is_err()
        );
        assert_eq!(
            scope.cancel(&b.task_id).await.unwrap().status,
            SubagentStatus::Cancelled
        );
        assert_eq!(
            scope.cancel(&a.task_id).await.unwrap().status,
            SubagentStatus::Cancelled
        );
        assert_eq!(gate.active.load(Ordering::SeqCst), 0);
        let c = scope.delegate(request("c"), "c").unwrap();
        started(&gate, 1).await;
        assert!(scope.delegate(request("limit"), "d").is_err());
        let tasks = scope.close().await;
        assert!(tasks.iter().all(|t| t.status.is_terminal()));
        assert_eq!(
            scope.get(&c.task_id).unwrap().status,
            SubagentStatus::Cancelled
        );
        assert!(scope.delegate(request("closed"), "e").is_err());
    }
    #[tokio::test]
    async fn execution_and_queue_timeouts_are_terminal_and_do_not_leak_slots() {
        let manager = Arc::new(
            SubagentManager::new(SubagentConfig {
                max_concurrent_tasks: 1,
                task_timeout_secs: 1,
                queue_timeout_secs: 1,
                ..Default::default()
            })
            .unwrap(),
        );
        let gate = Arc::new(Gate::new());
        let scope = manager.open_scope(owner(), gate.clone()).unwrap();
        let a = scope.delegate(request("a"), "a").unwrap();
        started(&gate, 1).await;
        let b = scope.delegate(request("b"), "b").unwrap();
        let result = scope
            .wait(&[a.task_id.clone(), b.task_id.clone()], WaitMode::All, 4)
            .await
            .unwrap();
        assert!(!result.timed_out);
        assert!(
            result
                .tasks
                .iter()
                .all(|t| t.status == SubagentStatus::TimedOut)
        );
        assert_eq!(scope.get(&a.task_id).unwrap().usage.total_tokens, 12);
        assert_eq!(gate.active.load(Ordering::SeqCst), 0);
        assert_eq!(manager.inner.slots.available_permits(), 1);
    }
    #[tokio::test]
    async fn queue_timeout_does_not_cancel_another_running_task() {
        let manager = Arc::new(
            SubagentManager::new(SubagentConfig {
                max_concurrent_tasks: 1,
                task_timeout_secs: 10,
                queue_timeout_secs: 1,
                ..Default::default()
            })
            .unwrap(),
        );
        let gate = Arc::new(Gate::new());
        let scope = manager.open_scope(owner(), gate.clone()).unwrap();
        let running = scope.delegate(request("running"), "first").unwrap();
        started(&gate, 1).await;
        let queued = scope.delegate(request("queued"), "second").unwrap();
        let result = scope
            .wait(&[queued.task_id], WaitMode::All, 3)
            .await
            .unwrap();
        assert!(!result.timed_out);
        assert_eq!(result.tasks[0].status, SubagentStatus::TimedOut);
        assert!(
            result.tasks[0]
                .error
                .as_ref()
                .unwrap()
                .contains("queue timeout")
        );
        assert_eq!(
            scope.get(&running.task_id).unwrap().status,
            SubagentStatus::Running
        );
        scope.close().await;
        assert_eq!(gate.active.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn errors_panics_shutdown_and_scope_drop_finish_tasks() {
        let manager = Arc::new(SubagentManager::new(SubagentConfig::default()).unwrap());
        let gate = Arc::new(Gate::new());
        let scope = manager.open_scope(owner(), gate.clone()).unwrap();
        let fail = scope.delegate(request("fail"), "fail").unwrap();
        let panic = scope.delegate(request("panic"), "panic").unwrap();
        gate.release.add_permits(2);
        let done = scope
            .wait(&[fail.task_id, panic.task_id], WaitMode::All, 2)
            .await
            .unwrap();
        assert!(
            done.tasks
                .iter()
                .all(|t| t.status == SubagentStatus::Failed)
        );
        let pending = scope.delegate(request("pending"), "pending").unwrap();
        started(&gate, 3).await;
        drop(scope);
        let entry = manager
            .entry(
                &TaskOwner {
                    session_id: pending.parent_session_id.clone(),
                    execution_id: pending.parent_execution_id.clone(),
                },
                &pending.task_id,
            )
            .unwrap();
        let cancelled = manager
            .wait_entries(&[entry], WaitMode::All, Some(Duration::from_secs(2)))
            .await;
        assert!(!cancelled.timed_out);
        assert_eq!(cancelled.tasks[0].status, SubagentStatus::Cancelled);
        manager.shutdown().await;
        assert!(manager.open_scope(owner(), gate).is_err());
    }
    #[tokio::test]
    async fn cancelled_child_session_is_persisted_with_parent_link() {
        let dir = std::env::temp_dir().join(format!("kyler-subagent-{}", Uuid::new_v4()));
        let store = Arc::new(SessionStore::new(&dir).await.unwrap());
        let manager = Arc::new(
            SubagentManager::new(Default::default())
                .unwrap()
                .with_session_store(store.clone()),
        );
        let gate = Arc::new(Gate::new());
        let scope = manager.open_scope(owner(), gate.clone()).unwrap();
        let task = scope.delegate(request("persist"), "call").unwrap();
        started(&gate, 1).await;
        scope.cancel(&task.task_id).await.unwrap();
        let session = store.load(&task.session_id).await.unwrap();
        assert_eq!(session.parent.as_ref().unwrap().task_id, task.task_id);
        assert_eq!(
            session.current_execution().unwrap().status,
            ExecutionStatus::Cancelled
        );
        assert_eq!(session.usage.total_tokens, 12);
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }
}
