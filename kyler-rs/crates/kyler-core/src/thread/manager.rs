use super::{NewThread, ThreadHandle, ThreadState, preferences, runner};
use crate::{
    Agent, Session, SessionStore, approval::ApprovalManager, attachments::AttachmentStore,
    environment::workspaces::WorkspaceFactory, memory::MemoryStore, processes::ProcessManager,
    questions::QuestionManager,
};

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use tokio::sync::{Mutex, RwLock, mpsc};

use kyler_config::AppConfig;

pub struct ThreadManager {
    config: Option<AppConfig>,
    pub(super) stopping: std::sync::atomic::AtomicBool,
    source: Option<PathBuf>,
    pub(super) store: Arc<SessionStore>,
    factory: Option<WorkspaceFactory>,
    pub(super) threads: Mutex<HashMap<String, ThreadHandle>>,
}
impl ThreadManager {
    /// Local sessions have their own storage namespace, independent of Web service sessions.
    pub async fn new(
        mut config: AppConfig,
        source: Option<PathBuf>,
        worker: PathBuf,
    ) -> Result<Self> {
        // Transport configuration is irrelevant to the local runtime.
        config.client = Default::default();
        config.server = Default::default();
        config.web = Default::default();
        config.session.enabled = true;
        config.session.directory = config
            .session
            .directory
            .parent()
            .context("session directory has no parent")?
            .join("tui/sessions");
        config.validate()?;
        let store = Arc::new(SessionStore::new(&config.session.directory).await?);
        let factory = WorkspaceFactory::configured(worker, Vec::new(), config.sandbox.clone())?;
        Ok(Self {
            stopping: std::sync::atomic::AtomicBool::new(false),
            config: Some(config),
            source,
            store,
            factory: Some(factory),
            threads: Mutex::new(HashMap::new()),
        })
    }
    /// Shared core manager for a host that already owns configuration and service resources.
    pub fn hosted(store: Arc<SessionStore>) -> Self {
        Self {
            config: None,
            source: None,
            store,
            factory: None,
            threads: Mutex::new(HashMap::new()),
            stopping: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub async fn get(&self, id: &str) -> Option<ThreadHandle> {
        self.threads.lock().await.get(id).cloned()
    }
    pub async fn close(&self, id: &str) -> Result<()> {
        let handle = self.threads.lock().await.remove(id);
        if let Some(handle) = handle {
            handle.shutdown().await?;
        }
        Ok(())
    }
    pub async fn start_thread(&self, cwd: &Path, resume: Option<&str>) -> Result<NewThread> {
        let mut threads = self.threads.lock().await;
        ensure!(
            !self.stopping.load(std::sync::atomic::Ordering::SeqCst),
            "thread manager is shutting down"
        );
        if let Some(handle) = resume.and_then(|id| threads.get(id)) {
            return Ok(NewThread {
                thread: handle.clone(),
                session_configured: handle.session().await,
            });
        }
        let config = self
            .config
            .as_ref()
            .context("hosted thread manager requires a host context")?;
        let factory = self
            .factory
            .as_ref()
            .context("local workspace factory is unavailable")?;
        let mut session = match resume {
            Some(id) => self.store.load(id).await?,
            None => Session::new(),
        };
        let environment = match &session.environment {
            Some(saved) => factory.restore(saved)?,
            None => Arc::new(factory.create(cwd)?.as_ref().clone().project_workspace()?),
        };
        session.environment = Some(environment.session_environment().clone());
        // A distinct lifetime lease excludes another TUI even while this thread is idle.
        let lock = crate::rollout::writer::private_file(
            &self
                .store
                .directory()
                .join(format!(".thread-{}.lock", session.session_id)),
            false,
        )?;
        lock.try_lock()
            .context("this session is already open in another TUI")?;
        session.recover_interrupted()?;
        self.store.save(&mut session).await?;
        let directory = self.store.directory().join(&session.session_id);
        let approvals = Arc::new(ApprovalManager::persistent(
            Duration::from_secs(300),
            directory.join("approvals"),
        )?);
        let questions = Arc::new(QuestionManager::persistent(directory.join("questions"))?);
        let processes = Arc::new(ProcessManager::persistent(directory.join("processes"))?);
        let attachments = Arc::new(AttachmentStore::new(
            self.store.directory().join("_attachments"),
        ));
        let memory = Arc::new(MemoryStore::new(self.store.directory().join("_memory")));
        let mut tools = crate::tools::build_toolbox().await?;
        factory.filter_tools(&mut tools);
        let agent =
            Agent::from_config_with_tools_and_store(config, Arc::new(tools), self.store.clone())
                .await?
                .with_environment(environment.clone())
                .with_questions(questions.clone(), None)
                .with_processes(processes.clone())
                .with_attachments(attachments.clone())
                .with_project_memory(memory, &session)
                .with_mcp_scope(&session.session_id);
        let preferences = directory.join("preferences.json");
        let (permissions, profile) = preferences::load(&preferences, config)?;
        ensure!(
            agent.model_registry().is_some(),
            "model registry is unavailable"
        );
        let (ops, receive_ops) = mpsc::channel(64);
        let (events, receive_events) = mpsc::channel(128);
        let handle = ThreadHandle {
            inner: Arc::new(ThreadState {
                agent,
                environment: RwLock::new(Some(environment)),
                session: RwLock::new(session.clone()),
                store: self.store.clone(),
                approvals,
                questions,
                processes,
                attachments,
                permissions: RwLock::new(permissions),
                default_approval: config.approval.clone(),
                profile: RwLock::new(profile),
                active: RwLock::new(None),
                cancel: std::sync::Mutex::new(None),
                source: self.source.clone(),
                preferences,
                hosted: false,
                enforce_permissions: true,
                event_timing: crate::diagnostics::ChannelTiming::new(
                    "runner_to_host",
                    &session.session_id,
                ),
                ops,
                events: Mutex::new(Some(receive_events)),
                task: Mutex::new(None),
            }),
        };
        let state = handle.inner.clone();
        *handle.inner.task.lock().await = Some(tokio::spawn(async move {
            let _lease = lock;
            let result = runner::run(state.clone(), receive_ops, events.clone()).await;
            state.agent.shutdown().await;
            state.processes.shutdown();
            let _ = events.send(super::ThreadEvent::ShutdownComplete).await;
            result
        }));
        threads.insert(session.session_id.clone(), handle.clone());
        Ok(NewThread {
            thread: handle,
            session_configured: session,
        })
    }
    pub async fn shutdown(&self) -> Result<()> {
        self.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let handles: Vec<_> = self.threads.lock().await.drain().map(|(_, h)| h).collect();
        let mut failure = None;
        for handle in handles {
            // An unclaimed stream must not block shutdown.
            handle.inner.events.lock().await.take();
            if let Err(error) = handle.shutdown().await {
                failure = Some(error);
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Drop for ThreadManager {
    fn drop(&mut self) {
        for thread in self.threads.get_mut().values() {
            if let Some((_, cancel)) = thread
                .inner
                .cancel
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                cancel.cancel();
            }
            let _ = thread.inner.ops.try_send(super::ThreadOp::Shutdown);
        }
    }
}
