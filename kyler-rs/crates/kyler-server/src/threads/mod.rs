//! HTTP-independent adaptation to the shared core operation/event API.
mod bridge;

use crate::{
    error::{ApiError, ApiResult},
    runs::RunManager,
};

use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, mpsc};

use kyler_core::{
    Session, SessionStore,
    thread::{HostedThread, ThreadEvent, ThreadHandle, ThreadManager},
};

#[derive(Clone)]
pub(crate) struct CoreThread {
    pub handle: ThreadHandle,
    pub events: Arc<Mutex<mpsc::Receiver<ThreadEvent>>>,
}
pub(crate) struct CoreThreads {
    manager: ThreadManager,
    threads: Mutex<HashMap<String, CoreThread>>,
}
impl CoreThreads {
    pub fn new(store: Arc<SessionStore>) -> Self {
        Self {
            manager: ThreadManager::hosted(store),
            threads: Mutex::new(HashMap::new()),
        }
    }
    async fn attach(&self, context: HostedThread) -> anyhow::Result<CoreThread> {
        let mut threads = self.threads.lock().await;
        let id = context.session.session_id.clone();
        if let Some(thread) = threads.get(&id) {
            if !thread.handle.is_closed() {
                return Ok(thread.clone());
            }
            threads.remove(&id);
            let _ = self.manager.close(&id).await;
        }
        let handle = self.manager.attach(context).await?;
        let events = Arc::new(Mutex::new(handle.take_events().await?));
        let thread = CoreThread { handle, events };
        threads.insert(id, thread.clone());
        Ok(thread)
    }
    pub async fn close(&self, id: &str) -> anyhow::Result<()> {
        self.threads.lock().await.remove(id);
        self.manager.close(id).await
    }
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.threads.lock().await.clear();
        self.manager.shutdown().await
    }
}
impl RunManager {
    pub(crate) async fn core_thread(&self, session: &Session) -> ApiResult<CoreThread> {
        self.threads
            .attach(HostedThread {
                agent: self.agent_for_session(session)?,
                session: session.clone(),
                approvals: self.approvals.clone(),
                questions: self.questions.clone(),
                processes: self.processes.clone(),
                attachments: self.attachments.clone(),
                preferences: self.root.clone(),
                default_approval: self.approval_settings.clone(),
                permissions: self.read_permissions(&session.session_id).await?,
                profile: self
                    .read_model_selection(&session.session_id)
                    .await?
                    .profile,
            })
            .await
            .map_err(ApiError::storage)
    }
}
