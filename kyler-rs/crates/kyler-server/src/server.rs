use crate::{routes, runs::RunManager, state::AppState, workspaces::WorkspaceFactory};

use std::{
    future::{Future, IntoFuture},
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::{Result, ensure};
use tokio::net::TcpListener;

use kyler_config::{AppConfig, ServerSettings};
use kyler_core::{
    Agent, ExecutionEnvironment, SessionStore,
    approval::ApprovalManager,
    tools::{ToolExecutionMode, build_toolbox, exec_command::ExecCommandTool, shell::ShellTool},
};

pub struct Server {
    pub state: AppState,
}
impl Server {
    pub async fn from_config(config: &AppConfig) -> Result<Self> {
        Self::configured(config, None, None).await
    }
    pub async fn from_config_with_environment(
        config: &AppConfig,
        environment: Arc<ExecutionEnvironment>,
    ) -> Result<Self> {
        Self::configured(config, Some(environment), None).await
    }
    pub async fn from_config_with_workspaces(
        config: &AppConfig,
        factory: Arc<WorkspaceFactory>,
    ) -> Result<Self> {
        ensure!(
            config.server.bind.ip().is_loopback() || !factory.unrestricted_roots(),
            "non-loopback workspace service requires an explicit allowed workspace root"
        );
        Self::configured(config, None, Some(factory)).await
    }
    async fn configured(
        config: &AppConfig,
        environment: Option<Arc<ExecutionEnvironment>>,
        factory: Option<Arc<WorkspaceFactory>>,
    ) -> Result<Self> {
        config.validate()?;
        ensure!(
            config.session.enabled,
            "kyler-server requires session.enabled=true"
        );
        let mut default_workspace = None;
        let factory = if let Some(factory) = factory {
            let mut settings = config.workspace.clone();
            let base = config
                .session
                .directory
                .parent()
                .unwrap_or_else(|| Path::new("."));
            settings.resolve_paths(base, std::env::var_os("HOME").as_deref().map(Path::new))?;
            tokio::fs::create_dir_all(&settings.default_directory).await?;
            let directory = settings.default_directory.canonicalize()?;
            let factory = factory
                .as_ref()
                .clone()
                .with_default_workspace(&directory)?;
            factory.create(&directory)?;
            default_workspace = Some(directory);
            Some(Arc::new(factory))
        } else {
            factory
        };
        let store = Arc::new(SessionStore::new(&config.session.directory).await?);
        kyler_protocol::startup::mark("storage_ready");
        let mut tools = build_toolbox().await?;
        if environment.is_none() && factory.is_none() {
            // Filesystem and command tools require explicit containment.
            tools.retain(|_, tool| tool.execution_mode() == ToolExecutionMode::Host);
        }
        if let Some(environment) = &environment {
            let policy = environment.policy();
            if policy.filesystem.readwrite_paths.is_empty() {
                tools.remove("apply_patch");
            }
            if policy.process.allow_commands {
                tools.insert("exec_command".into(), Arc::new(ExecCommandTool));
                tools.insert("shell".into(), Arc::new(ShellTool));
            }
        }
        if let Some(factory) = &factory {
            factory.filter_tools(&mut tools);
        }
        let mut agent =
            Agent::from_config_with_tools_and_store(config, Arc::new(tools), store.clone()).await?;
        if let Some(environment) = environment {
            agent = agent.with_environment(environment);
        }
        kyler_protocol::startup::mark("agent_ready");
        let mut server = Self::with_agent(config.server.clone(), Arc::new(agent), store).await?;
        let runs =
            Arc::get_mut(&mut server.state.runs).expect("new manager has no external owners");
        runs.workspaces = factory;
        runs.default_workspace =
            default_workspace.or_else(|| runs.agent.workspace_environment().map(|e| e.cwd.clone()));
        runs.approval_settings = config.approval.clone();
        for path in [&config.skills.state_file, &config.skills.user_directory] {
            if path.is_absolute() {
                runs.protected_paths.push(path.clone());
            }
        }
        runs.approvals = Some(Arc::new(ApprovalManager::persistent(
            Duration::from_secs(config.server.approval_timeout_seconds),
            config.session.directory.join("_approvals"),
        )?));
        let database = config
            .session
            .directory
            .parent()
            .unwrap_or(Path::new("."))
            .join("scheduler.sqlite3");
        for suffix in ["", "-wal", "-shm"] {
            runs.protected_paths.push(std::path::PathBuf::from(format!(
                "{}{suffix}",
                database.display()
            )));
        }
        let channel_database = config.session.directory.join("_channels.sqlite3");
        let channel_settings = config
            .channels
            .iter()
            .map(|(key, value)| (key.clone(), value.expose().to_owned()))
            .collect();
        let settings = kyler_channel::Settings(&channel_settings);
        server.state.channels =
            crate::channels::Channels::configured(&channel_database, &settings)?.map(Arc::new);
        if settings.get("KYLER_GOOGLE_CHAT_WEBHOOK_URL").is_some() {
            let path = settings.required("KYLER_GOOGLE_CHAT_CREDENTIALS_FILE")?;
            runs.protected_paths.push(
                Path::new(&path)
                    .canonicalize()
                    .map_err(|_| anyhow::anyhow!("Cannot protect Google Chat credentials"))?,
            );
        }
        if server.state.channels.is_some() {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let mut path = channel_database.clone().into_os_string();
                path.push(suffix);
                runs.protected_paths.push(path.into());
            }
        }
        let binding = runs
            .store
            .directory()
            .canonicalize()?
            .to_string_lossy()
            .into_owned();
        let concurrency = runs.settings.max_concurrent_runs.min(2);
        server.state.scheduler = Some(
            kyler_scheduler::Scheduler::open(
                &database,
                &binding,
                Arc::new(crate::automations::AutomationExecutor {
                    manager: Arc::downgrade(&server.state.runs),
                }),
                concurrency,
            )
            .await?,
        );
        kyler_protocol::startup::mark("server_state_ready");
        Ok(server)
    }
    /// Trusted embedding API. The caller supplies its authorized tools/environment.
    pub async fn with_agent(
        settings: ServerSettings,
        agent: Arc<Agent>,
        store: Arc<SessionStore>,
    ) -> Result<Self> {
        let agent = Arc::new(agent.as_ref().clone().with_session_store(store.clone()));
        Ok(Self {
            state: AppState {
                instance_id: uuid::Uuid::new_v4().to_string(),
                runs: RunManager::new(agent, store, settings).await?,
                scheduler: None,
                channels: None,
            },
        })
    }
    pub fn router(&self) -> axum::Router {
        routes::router(self.state.clone())
    }
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()> + Send,
    ) -> Result<()> {
        let manager = self.state.runs.clone();
        let address = listener.local_addr()?;
        ensure!(
            manager
                .security
                .store
                .list()
                .await?
                .iter()
                .any(
                    |token| token.role == kyler_protocol::api::gateway::Role::Owner
                        && token.revoked_at.is_none()
                        && token
                            .expires_at
                            .is_none_or(|expiry| expiry > chrono::Utc::now().timestamp_millis())
                ),
            "No active gateway owner token. Run kyler configure or kyler token create --name owner"
        );
        ensure!(
            listener.local_addr()?.ip().is_loopback()
                || manager
                    .workspaces
                    .as_ref()
                    .is_none_or(|factory| !factory.unrestricted_roots()),
            "non-loopback workspace listener requires explicit allowed roots"
        );
        let scheduler = if let Some(scheduler) = &self.state.scheduler {
            Some(scheduler.start().await?)
        } else {
            None
        };
        let channel_worker = async {
            if let Some(channels) = &self.state.channels {
                channels.run(&manager).await;
            }
            std::future::pending::<()>().await;
        };
        tokio::pin!(channel_worker);
        let mut stopping = manager.stopping.subscribe();
        let http = axum::serve(listener, self.router())
            .with_graceful_shutdown(async move {
                let _ = stopping.wait_for(|v| *v).await;
            })
            .into_future();
        tokio::pin!(http);
        kyler_protocol::startup::emit(
            "service_listening",
            serde_json::json!({"address":address.to_string()}),
        );
        let cleaner = async {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                if manager.cleanup().await.is_err() {
                    tracing::warn!("run retention cleanup failed");
                }
            }
        };
        tokio::pin!(cleaner);
        tokio::select! {
            result=&mut http=>{if let Some(scheduler)=scheduler {let _=scheduler.stop().await;} manager.shutdown().await;result?;}
            _=shutdown=>{
                if let Some(scheduler)=scheduler {let _=scheduler.stop().await;}
                let deadline=tokio::time::Instant::now()+Duration::from_secs(manager.settings.shutdown_grace_seconds);
                let clean=manager.shutdown().await;
                if !clean {tracing::warn!("shutdown grace period expired; incomplete runs will be recovered on restart");}
                tokio::time::timeout_at(deadline,&mut http).await??;
            }
            _=&mut cleaner=>unreachable!(),
            _=&mut channel_worker=>unreachable!(),
        }
        Ok(())
    }
}
