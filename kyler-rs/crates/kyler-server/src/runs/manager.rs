//! Runtime service construction and session workspace binding.
use super::{events::Entry, persistence, registry::Registry};
use crate::{
    error::{ApiError, ApiResult, valid_id},
    workspaces::WorkspaceFactory,
};

use std::{
    cmp::Reverse,
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use tokio::sync::{Mutex, Semaphore, watch};

use kyler_config::{ServerSettings, loader::LlmOverrides};
use kyler_core::{
    Agent, ExecutionStatus, Session, SessionEnvironment, SessionStore, TokenUsage,
    approval::ApprovalManager, attachments::AttachmentStore, memory::MemoryStore,
    processes::ProcessManager,
};
use kyler_protocol::{
    api::{
        ApiErrorBody,
        run::{RunAccepted, RunInfo, RunStatus, Usage},
    },
    approval::ApprovalSettings,
};

pub struct RunManager {
    pub(crate) threads: crate::threads::CoreThreads,
    pub(crate) questions: Arc<kyler_core::questions::QuestionManager>,
    pub(crate) approval_settings: ApprovalSettings,
    pub(crate) processes: Arc<ProcessManager>,
    pub(crate) memories: Arc<MemoryStore>,
    pub(crate) attachments: Arc<AttachmentStore>,
    pub(crate) artifacts: Arc<kyler_core::artifacts::ArtifactStore>,
    pub(crate) approvals: Option<Arc<ApprovalManager>>,
    pub(crate) protected_paths: Vec<PathBuf>,
    pub(crate) agent: Arc<Agent>,
    pub(crate) workspaces: Option<Arc<WorkspaceFactory>>,
    pub(crate) default_workspace: Option<PathBuf>,
    pub store: Arc<SessionStore>,
    pub settings: ServerSettings,
    pub(crate) registry: Mutex<Registry>,
    pub(crate) permits: Arc<Semaphore>,
    pub stopping: watch::Sender<bool>,
    pub(crate) root: PathBuf,
    pub(crate) model_reload: Mutex<()>,
    pub(crate) workspace_catalog: Mutex<()>,
    pub(crate) model_source: RwLock<Option<(PathBuf, LlmOverrides)>>,
    pub(crate) _lock: File,
}
impl RunManager {
    pub async fn new(
        agent: Arc<Agent>,
        store: Arc<SessionStore>,
        settings: ServerSettings,
    ) -> anyhow::Result<Arc<Self>> {
        settings.validate()?;
        let root = store.directory().join("_runs");
        let (lock, mut manifests) = persistence::initialize(root.clone()).await?;
        let mut runs = HashMap::new();
        let now = chrono::Utc::now().timestamp_millis();
        manifests.sort_by_key(|r| Reverse(r.created_at));
        for mut info in manifests {
            if !info.status.is_terminal() {
                // SessionStore repairs orphaned calls. Never resubmit the recorded input.
                let session = store.load(&info.session_id).await?;
                anyhow::ensure!(
                    !session.is_running(),
                    "a recorded run is still owned by an active external execution"
                );
                let execution = info.execution_id.as_ref().and_then(|id| {
                    session
                        .execution
                        .iter()
                        .chain(session.executions.iter())
                        .find(|e| &e.execution_id == id)
                });
                if execution.is_some_and(|e| e.status == ExecutionStatus::Completed) {
                    let execution = execution.unwrap();
                    info.status = RunStatus::Completed;
                    info.output = execution.final_result.clone();
                    info.usage = Some(usage(execution.usage));
                } else {
                    info.status = RunStatus::Failed;
                    info.error = Some(public_error(
                        "server_restarted",
                        "The execution was interrupted by a server restart; tool outcomes may be unknown.",
                    ));
                }
                info.finished_at = Some(now);
                persistence::save(root.clone(), info.clone()).await?;
            }
            let expired = info.finished_at.is_some_and(|t| {
                now.saturating_sub(t) > settings.completed_run_retention_seconds as i64 * 1000
            });
            if expired || runs.len() >= settings.max_retained_runs {
                persistence::remove(&root, &info.run_id).await?;
                continue;
            }
            runs.insert(
                info.run_id.clone(),
                Arc::new(Entry::new(
                    info,
                    settings.event_buffer_max_events,
                    settings.event_buffer_max_bytes,
                    true,
                    settings.max_subscribers_per_run,
                )),
            );
        }
        let mut keys = HashMap::new();
        for admission in persistence::admissions(root.clone()).await? {
            keys.insert(
                (admission.session_id, admission.key),
                (admission.run_id, admission.fingerprint),
            );
        }
        let attachments = Arc::new(AttachmentStore::new(store.directory().join("_attachments")));
        let processes = Arc::new(ProcessManager::persistent(
            store.directory().join("_processes"),
        )?);
        let agent = Arc::new(
            agent
                .as_ref()
                .clone()
                .with_attachments(attachments.clone())
                .with_processes(processes.clone()),
        );
        Ok(Arc::new(Self {
            threads: crate::threads::CoreThreads::new(store.clone()),
            questions: Arc::new(kyler_core::questions::QuestionManager::persistent(
                store.directory().join("_questions"),
            )?),
            approval_settings: Default::default(),
            processes,
            memories: Arc::new(MemoryStore::new(store.directory().join("_memory"))),
            artifacts: Arc::new(kyler_core::artifacts::ArtifactStore::new(
                store.directory().join("_artifacts"),
            )),
            attachments,
            approvals: None,
            protected_paths: Vec::new(),
            agent,
            workspaces: None,
            default_workspace: None,
            store,
            permits: Arc::new(Semaphore::new(settings.max_concurrent_runs)),
            settings,
            registry: Mutex::new(Registry {
                runs,
                active: HashMap::new(),
                keys,
            }),
            stopping: watch::channel(false).0,
            model_reload: Mutex::new(()),
            workspace_catalog: Mutex::new(()),
            model_source: RwLock::new(None),
            root,
            _lock: lock,
        }))
    }
    pub fn environment_for_create(
        &self,
        workspace: Option<&str>,
    ) -> ApiResult<Option<SessionEnvironment>> {
        if let Some(factory) = &self.workspaces {
            let default = self.default_workspace.as_deref().and_then(Path::to_str);
            let path = workspace.or(default).ok_or_else(|| {
                ApiError::bad(
                    "workspace_root is required; create the session from its project directory.",
                )
            })?;
            let environment=factory.create(Path::new(path)).map_err(|_|ApiError::bad("Workspace must be an existing absolute directory inside the server's allowed roots."))?;
            return Ok(Some(environment.session_environment().clone()));
        }
        let active = self.agent.workspace_environment();
        if let Some(requested) = workspace {
            if active.is_none_or(|snapshot| snapshot.workspace_root != Path::new(requested)) {
                return Err(ApiError::bad(
                    "This server does not authorize the requested session workspace.",
                ));
            }
        }
        Ok(active.cloned())
    }
    pub(crate) fn agent_for_session(&self, session: &Session) -> ApiResult<Agent> {
        let mut agent = self
            .agent
            .as_ref()
            .clone()
            .with_project_memory(self.memories.clone(), session);
        if let Some(environment) = self.isolated_environment(session)? {
            agent = agent.with_environment(environment);
        }
        if self.approvals.is_some() {
            if let Some(environment) = agent.execution_environment() {
                let mut protected = self.protected_paths.clone();
                // Private execution is inside the store, but its parent metadata is not readable.
                if environment.session_environment().project_root.is_none() {
                    protected.push(self.store.directory().to_path_buf());
                }
                if let Some(project) = &environment.session_environment().project_root {
                    if let Ok(relative) = self.store.directory().strip_prefix(project) {
                        protected.push(
                            environment
                                .session_environment()
                                .workspace_root
                                .join(relative),
                        );
                    }
                }
                protected.retain(|path| {
                    !environment
                        .session_environment()
                        .workspace_root
                        .starts_with(path)
                });
                if let Some((source, _)) =
                    &*self.model_source.read().map_err(|_| ApiError::internal())?
                {
                    let source = if source.is_absolute() {
                        source.clone()
                    } else {
                        std::env::current_dir()
                            .map_err(|_| ApiError::internal())?
                            .join(source)
                    };
                    if let Some(parent) = source.parent() {
                        let base = if parent.file_name().is_some_and(|n| n == "config") {
                            parent.parent().unwrap_or(parent)
                        } else {
                            parent
                        };
                        protected.push(base.join(".env"));
                    }
                    protected.push(source);
                }
                agent = agent.with_environment(Arc::new(
                    environment
                        .with_protected_paths(&protected)
                        .map_err(|_| ApiError::bad("Unable to protect server control files."))?,
                ));
            }
        }
        Ok(agent.with_mcp_scope(session.session_id.clone()))
    }
    pub async fn entry(&self, id: &str) -> ApiResult<Arc<Entry>> {
        valid_id(id)?;
        self.registry
            .lock()
            .await
            .runs
            .get(id)
            .cloned()
            .ok_or_else(ApiError::missing)
    }
}
pub(super) fn public_error(code: &str, message: &str) -> ApiErrorBody {
    ApiErrorBody {
        code: code.into(),
        message: message.into(),
    }
}
pub(super) fn accepted(info: &RunInfo) -> RunAccepted {
    RunAccepted {
        run_id: info.run_id.clone(),
        session_id: info.session_id.clone(),
        status: info.status,
        events_url: format!("/v1/runs/{}/events", info.run_id),
    }
}
pub(super) fn usage(value: TokenUsage) -> Usage {
    Usage {
        prompt_tokens: value.prompt_tokens,
        completion_tokens: value.completion_tokens,
        total_tokens: value.total_tokens,
    }
}
