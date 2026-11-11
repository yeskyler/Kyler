//! Terminal adapter to local core threads. Never opens a backend connection.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use tokio::sync::{Mutex, OnceCell, mpsc};

use kyler_config::AppConfig;
use kyler_core::{
    runtime::UserTurn,
    thread::{ThreadEvent, ThreadHandle, ThreadManager},
};
use kyler_protocol::api::{
    attachments::AttachmentInfo,
    history::HistoryPage,
    mcp::*,
    models::{ModelCatalog, SessionModel},
    permissions::*,
    questions::{QuestionRecord, QuestionResponse},
    session::SessionSummary,
    skills::*,
    tools::ToolDetails,
    *,
};

#[derive(Clone)]
pub struct AgentHandle {
    inner: Arc<State>,
}
struct State {
    config: AppConfig,
    source: Option<PathBuf>,
    manager: OnceCell<ThreadManager>,
    threads: Mutex<HashMap<String, LocalThread>>,
    turns: Mutex<HashMap<String, String>>,
}
#[derive(Clone)]
struct LocalThread {
    handle: ThreadHandle,
    events: Arc<Mutex<mpsc::Receiver<ThreadEvent>>>,
}
#[derive(Debug, Clone)]
pub enum AgentEvent {
    Event(event::ServerEvent),
    Result(run::RunInfo),
}
pub struct TurnEvents {
    events: Arc<Mutex<mpsc::Receiver<ThreadEvent>>>,
}
impl TurnEvents {
    pub async fn next(&mut self) -> Result<Option<AgentEvent>> {
        Ok(match self.events.lock().await.recv().await {
            Some(ThreadEvent::Event(event)) => Some(AgentEvent::Event(event::ServerEvent {
                run_id: event.op_id,
                execution_id: event.execution_id,
                seq: event.seq,
                timestamp_ms: event.timestamp_ms,
                payload: event.payload,
            })),
            Some(ThreadEvent::TurnFinished(info)) => Some(AgentEvent::Result(info)),
            Some(ThreadEvent::ShutdownComplete) | None => None,
        })
    }
}
impl AgentHandle {
    pub fn new(config: AppConfig, source: Option<PathBuf>) -> Self {
        Self {
            inner: Arc::new(State {
                config,
                source,
                manager: OnceCell::new(),
                threads: Mutex::new(HashMap::new()),
                turns: Mutex::new(HashMap::new()),
            }),
        }
    }
    async fn manager(&self) -> Result<&ThreadManager> {
        self.inner
            .manager
            .get_or_try_init(|| async {
                ThreadManager::new(
                    self.inner.config.clone(),
                    self.inner.source.clone(),
                    std::env::current_exe()?,
                )
                .await
            })
            .await
    }
    async fn thread(&self, id: &str) -> Result<ThreadHandle> {
        if let Some(thread) = self.inner.threads.lock().await.get(id) {
            return Ok(thread.handle.clone());
        }
        let new = self
            .manager()
            .await?
            .start_thread(&std::env::current_dir()?, Some(id))
            .await?;
        self.register(new.thread).await
    }
    async fn register(&self, handle: ThreadHandle) -> Result<ThreadHandle> {
        let mut threads = self.inner.threads.lock().await;
        let id = handle.session().await.session_id;
        if let Some(existing) = threads.get(&id) {
            return Ok(existing.handle.clone());
        }
        let events = Arc::new(Mutex::new(handle.take_events().await?));
        threads.insert(
            id,
            LocalThread {
                handle: handle.clone(),
                events,
            },
        );
        Ok(handle)
    }
    pub async fn shutdown(&self) -> Result<()> {
        // Drain on shutdown so a full UI queue cannot delay cooperative cancellation.
        let threads: Vec<_> = self
            .inner
            .threads
            .lock()
            .await
            .drain()
            .map(|(_, t)| t)
            .collect();
        let mut drains = Vec::new();
        for thread in threads {
            drains.push(tokio::spawn(async move {
                let mut receiver = thread.events.lock().await;
                while receiver.recv().await.is_some() {}
            }));
        }
        let result = match self.inner.manager.get() {
            Some(manager) => manager.shutdown().await,
            None => Ok(()),
        };
        for drain in drains {
            let _ = drain.await;
        }
        result
    }
    pub async fn info(&self, id: &str) -> Result<ServerInfo> {
        let thread = self.thread(id).await?;
        let profile = thread.model(None).await?.profile;
        let agent = thread.agent().bind_model(profile.as_deref())?;
        Ok(ServerInfo {
            instance_id: None,
            model: agent.model().into(),
            context_window_tokens: agent.context_window_tokens(),
            sliding_window_tokens: Some(agent.sliding_window_tokens()),
            default_workspace: None,
            workspace_root: None,
            working_directory: None,
        })
    }
    pub async fn create_project_session_in(&self, cwd: &Path) -> Result<SessionSummary> {
        let new = self.manager().await?.start_thread(cwd, None).await?;
        let handle = self.register(new.thread).await?;
        summary(&handle).await
    }
    pub async fn session(&self, id: &str) -> Result<SessionSummary> {
        summary(&self.thread(id).await?).await
    }
    pub async fn session_usage(&self, id: &str) -> Result<SessionUsage> {
        Ok(self.thread(id).await?.usage().await)
    }
    pub async fn history(&self, id: &str, cursor: usize, limit: usize) -> Result<HistoryPage> {
        self.thread(id).await?.history(cursor, limit).await
    }
    pub async fn tool_details(&self, id: &str, call: &str) -> Result<ToolDetails> {
        self.thread(id).await?.tool_details(call).await
    }
    pub async fn upload_attachment(
        &self,
        id: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<AttachmentInfo> {
        let thread = self.thread(id).await?;
        let name = name.to_owned();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || thread.attachments().put(&id, &name, &bytes)).await?
    }
    pub async fn start_turn(
        &self,
        id: &str,
        text: String,
        attachments: Vec<AttachmentInfo>,
        resume: bool,
    ) -> Result<String> {
        let thread = self.thread(id).await?;
        let run = thread
            .submit_turn(UserTurn {
                text,
                attachments,
                resume,
                ..Default::default()
            })
            .await?;
        let mut turns = self.inner.turns.lock().await;
        turns.retain(|_, session| session != id);
        turns.insert(run.clone(), id.to_owned());
        Ok(run)
    }
    pub async fn watch(&self, run: &str) -> Result<TurnEvents> {
        let id = self
            .inner
            .turns
            .lock()
            .await
            .get(run)
            .cloned()
            .context("local turn is unavailable")?;
        let thread = self
            .inner
            .threads
            .lock()
            .await
            .get(&id)
            .cloned()
            .context("local thread is unavailable")?;
        Ok(TurnEvents {
            events: thread.events,
        })
    }
    pub async fn cancel(&self, run: &str) -> Result<()> {
        let id = self
            .inner
            .turns
            .lock()
            .await
            .get(run)
            .cloned()
            .context("local turn is unavailable")?;
        self.thread(&id).await?.cancel().await
    }
    pub async fn models(&self, reload: bool) -> Result<ModelCatalog> {
        let thread = self
            .inner
            .threads
            .lock()
            .await
            .values()
            .next()
            .cloned()
            .context("no local thread")?;
        thread.handle.models(reload).await
    }
    pub async fn session_model(&self, id: &str) -> Result<SessionModel> {
        self.thread(id).await?.model(None).await
    }
    pub async fn select_model(&self, id: &str, profile: Option<String>) -> Result<SessionModel> {
        self.thread(id).await?.model(Some(profile)).await
    }
    pub async fn permissions(&self, id: &str) -> Result<PermissionState> {
        self.thread(id).await?.permissions(None).await
    }
    pub async fn select_permissions(
        &self,
        id: &str,
        selection: &PermissionSelection,
    ) -> Result<PermissionState> {
        self.thread(id).await?.permissions(Some(selection)).await
    }
    pub async fn approvals(&self, id: &str) -> Result<Vec<ApprovalRecord>> {
        Ok(self.thread(id).await?.approvals().list(id))
    }
    pub async fn decide_approval(
        &self,
        id: &str,
        response: &ApprovalResponse,
    ) -> Result<ApprovalRecord> {
        let threads = self.inner.threads.lock().await;
        for thread in threads.values() {
            if thread
                .handle
                .approvals()
                .list(&thread.handle.session().await.session_id)
                .iter()
                .any(|r| r.id == id)
            {
                return thread
                    .handle
                    .respond_approval(id.into(), response.clone())
                    .await;
            }
        }
        anyhow::bail!("approval is unavailable or expired")
    }
    pub async fn questions(&self, id: &str) -> Result<Vec<QuestionRecord>> {
        Ok(self.thread(id).await?.questions().list(id))
    }
    pub async fn answer_question(
        &self,
        id: &str,
        question: &str,
        response: &QuestionResponse,
    ) -> Result<QuestionRecord> {
        self.thread(id)
            .await?
            .respond_question(id.into(), question.into(), response.clone())
            .await
    }
    pub async fn skills(&self, id: &str, reload: bool) -> Result<Vec<SkillInfo>> {
        let thread = self.thread(id).await?;
        let agent = thread.agent();
        match agent.skill_manager() {
            Some(m) => {
                m.list(
                    agent
                        .workspace_environment()
                        .map(|e| e.workspace_root.as_path()),
                    reload,
                )
                .await
            }
            None => Ok(Vec::new()),
        }
    }
    pub async fn skill_details(&self, id: &str, skill: &str) -> Result<SkillDetails> {
        let thread = self.thread(id).await?;
        let agent = thread.agent();
        agent
            .skill_manager()
            .context("skills are disabled")?
            .details(
                agent
                    .workspace_environment()
                    .map(|e| e.workspace_root.as_path()),
                skill,
            )
            .await
    }
    pub async fn skill_action(
        &self,
        id: &str,
        skill: &str,
        action: SkillAction,
        fingerprint: String,
    ) -> Result<()> {
        let thread = self.thread(id).await?;
        let agent = thread.agent();
        agent
            .skill_manager()
            .context("skills are disabled")?
            .action(
                agent
                    .workspace_environment()
                    .map(|e| e.workspace_root.as_path()),
                skill,
                action,
                &fingerprint,
            )
            .await
    }
    pub async fn mcp_changes(&self, id: &str) -> Result<tokio::sync::watch::Receiver<u64>> {
        let thread = self.thread(id).await?;
        Ok(thread
            .agent()
            .mcp_manager()
            .context("MCP is unavailable")?
            .subscribe())
    }
    pub async fn mcp_servers(&self, id: &str) -> Result<Vec<McpServerInfo>> {
        let thread = self.thread(id).await?;
        let agent = thread.agent();
        match agent.mcp_manager() {
            Some(m) => m.list(id, agent.execution_environment()).await,
            None => Ok(Vec::new()),
        }
    }
    pub async fn mcp_action(&self, id: &str, server: &str, action: McpAction) -> Result<()> {
        let thread = self.thread(id).await?;
        let agent = thread.agent();
        agent
            .mcp_manager()
            .context("MCP is unavailable")?
            .action(id, agent.execution_environment(), server, action)
            .await
    }
}
async fn summary(thread: &ThreadHandle) -> Result<SessionSummary> {
    let s = thread.session().await;
    Ok(SessionSummary {
        draft: false,
        title: None,
        session_id: s.session_id,
        created_at: s.created_at,
        updated_at: s.updated_at,
        event_count: s.events.len(),
        execution_count: s.executions.len() + usize::from(s.execution.is_some()),
        status: None,
        workspace_root: s
            .environment
            .as_ref()
            .map(|e| e.workspace_root.display().to_string()),
        working_directory: s
            .environment
            .map(|e| e.working_directory.display().to_string()),
    })
}
