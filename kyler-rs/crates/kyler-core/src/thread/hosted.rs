//! Attach host-authorized sessions without taking ownership of shared service resources.
use super::{ThreadEvent, ThreadHandle, ThreadManager, ThreadState};
use crate::{
    Agent, Session, approval::ApprovalManager, attachments::AttachmentStore,
    processes::ProcessManager, questions::QuestionManager,
};

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, ensure};
use tokio::sync::{Mutex, RwLock, mpsc};

use kyler_protocol::api::permissions::PermissionSnapshot;

pub struct HostedThread {
    pub agent: Agent,
    pub session: Session,
    pub approvals: Option<Arc<ApprovalManager>>,
    pub questions: Arc<QuestionManager>,
    pub processes: Arc<ProcessManager>,
    pub attachments: Arc<AttachmentStore>,
    /// Directory of existing service preference files; the on-disk format is preserved.
    pub preferences: PathBuf,
    pub permissions: PermissionSnapshot,
    pub default_approval: kyler_protocol::approval::ApprovalSettings,
    pub profile: Option<String>,
}
impl ThreadManager {
    pub async fn attach(&self, context: HostedThread) -> Result<ThreadHandle> {
        let mut threads = self.threads.lock().await;
        ensure!(
            !self.stopping.load(std::sync::atomic::Ordering::SeqCst),
            "thread manager is shutting down"
        );
        if let Some(handle) = threads.get(&context.session.session_id) {
            return Ok(handle.clone());
        }
        context.session.validate()?;
        let lease = crate::rollout::writer::private_file(
            &self
                .store
                .directory()
                .join(format!(".thread-{}.lock", context.session.session_id)),
            false,
        )?;
        lease
            .try_lock()
            .context("session is already attached to another thread host")?;
        let (ops, receive_ops) = mpsc::channel(64);
        let (events, receive_events) = mpsc::channel(128);
        let enforce_permissions = context.approvals.is_some();
        let id = context.session.session_id.clone();
        let environment = context.agent.execution_environment();
        let handle = ThreadHandle {
            inner: Arc::new(ThreadState {
                agent: context.agent,
                environment: RwLock::new(environment),
                session: RwLock::new(context.session),
                store: self.store.clone(),
                approvals: context
                    .approvals
                    .unwrap_or_else(|| Arc::new(ApprovalManager::new(Duration::from_secs(300)))),
                questions: context.questions,
                processes: context.processes,
                attachments: context.attachments,
                permissions: RwLock::new(context.permissions),
                default_approval: context.default_approval,
                profile: RwLock::new(context.profile),
                active: RwLock::new(None),
                cancel: std::sync::Mutex::new(None),
                source: None,
                preferences: context.preferences,
                hosted: true,
                enforce_permissions,
                event_timing: crate::diagnostics::ChannelTiming::new("runner_to_host", &id),
                ops,
                events: Mutex::new(Some(receive_events)),
                task: Mutex::new(None),
            }),
        };
        let state = handle.inner.clone();
        *handle.inner.task.lock().await = Some(tokio::spawn(async move {
            let _lease = lease;
            // The service owns the shared MCP/process managers and shuts them down once.
            let result = super::runner::run(state, receive_ops, events.clone()).await;
            let _ = events.send(ThreadEvent::ShutdownComplete).await;
            result
        }));
        threads.insert(id, handle.clone());
        Ok(handle)
    }
}
