use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use kyler_config::{AppConfig, SandboxSettings};
use kyler_core::{
    Agent, SessionStore,
    approval::{ApprovalContext, ApprovalManager},
    attachments::AttachmentStore,
    mcp::McpManager,
    processes::ProcessManager,
    runtime::observation::{RuntimeObservation, RuntimeObserver},
};
use kyler_protocol::{
    api::permissions::{PermissionMode, PermissionSnapshot},
    approval::{ApprovalPolicy, ToolRule},
    sandbox::SandboxMode,
};
use kyler_server::workspaces::WorkspaceFactory;

use crate::config::EvalConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TimingData {
    pub model_requests: usize,
    pub first_request_ms: Option<f64>,
    pub first_model_event_ms: Option<f64>,
    pub mcp_snapshot_ms: Vec<f64>,
    pub skills_snapshot_ms: Vec<f64>,
    pub tool_snapshots: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub capabilities_ready_ms: Option<f64>,
}
pub struct Timings {
    started: Instant,
    data: Mutex<TimingData>,
    startup_trace: bool,
}
impl Timings {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Instant::now(),
            data: Mutex::new(TimingData::default()),
            startup_trace: false,
        })
    }
    pub fn startup() -> Arc<Self> {
        Arc::new(Self {
            started: Instant::now(),
            data: Mutex::new(TimingData::default()),
            startup_trace: true,
        })
    }
    pub fn snapshot(&self) -> TimingData {
        self.data.lock().unwrap().clone()
    }
}
impl RuntimeObserver for Timings {
    fn observe(&self, observation: RuntimeObservation) {
        let elapsed = self.started.elapsed().as_secs_f64() * 1000.0;
        let mut data = self.data.lock().unwrap();
        match observation {
            RuntimeObservation::ModelRequestStarted => {
                if self.startup_trace && data.first_request_ms.is_none() {
                    kyler_protocol::startup::mark("model_request_started");
                }
                data.model_requests += 1;
                data.first_request_ms.get_or_insert(elapsed);
            }
            RuntimeObservation::FirstModelEvent => {
                if self.startup_trace && data.first_model_event_ms.is_none() {
                    kyler_protocol::startup::mark("first_model_event");
                }
                data.first_model_event_ms.get_or_insert(elapsed);
            }
            RuntimeObservation::McpSnapshot { elapsed } => {
                data.mcp_snapshot_ms.push(elapsed.as_secs_f64() * 1000.0)
            }
            RuntimeObservation::SkillSnapshot { elapsed } => {
                data.skills_snapshot_ms.push(elapsed.as_secs_f64() * 1000.0)
            }
            RuntimeObservation::ToolSnapshot { fingerprint, names } => {
                if data.capabilities_ready_ms.is_none() {
                    data.capabilities_ready_ms = Some(elapsed);
                    if self.startup_trace {
                        kyler_protocol::startup::mark("capabilities_ready");
                    }
                }
                data.tool_snapshots.insert(fingerprint, names);
            }
        }
    }
}
pub struct Runtime {
    pub agent: Agent,
    pub attachments: Arc<AttachmentStore>,
    pub processes: Arc<ProcessManager>,
    pub store: Arc<SessionStore>,
    pub workspace: tempfile::TempDir,
    pub phases_ms: BTreeMap<String, f64>,
}
impl Runtime {
    pub async fn build(
        app: &AppConfig,
        eval: &EvalConfig,
        private: &Path,
        worker: &Path,
    ) -> Result<Self> {
        let workspace = tempfile::Builder::new()
            .prefix("kyler-eval-work-")
            .tempdir()?;
        let mut phases_ms = BTreeMap::new();
        let started = Instant::now();
        let sandbox = SandboxSettings {
            mode: SandboxMode::WorkspaceWrite,
            network_access: app.sandbox.network_access,
            allow_commands: app.sandbox.allow_commands,
            ..Default::default()
        };
        let environment =
            WorkspaceFactory::configured(worker.into(), vec![workspace.path().into()], sandbox)?
                .create(workspace.path())?;
        let mut denied = vec![eval.run.results_directory.canonicalize()?];
        if eval.dataset.directory.exists() {
            denied.push(eval.dataset.directory.canonicalize()?);
        }
        if let Some(home) = std::env::var_os("HOME") {
            denied.push(PathBuf::from(home).join(".kyler"));
        }
        let environment = Arc::new(environment.with_protected_paths(&denied)?);
        phases_ms.insert(
            "sandbox_and_runtime_discovery".into(),
            started.elapsed().as_secs_f64() * 1000.0,
        );
        let started = Instant::now();
        let mut config = app.clone();
        config.session.enabled = true;
        config.session.directory = private.join("sessions");
        config.agent.max_steps = eval.run.max_steps;
        config.skills.enabled = eval.agent.load_user_skills;
        config.skills.state_file = private.join("skills-state.json");
        if eval.agent.load_user_skills {
            config.skills.user_directory = private.join("skills");
            skill_tree(
                &app.skills.user_directory,
                Some(&config.skills.user_directory),
            )?;
            if app.skills.state_file.is_file() {
                std::fs::copy(&app.skills.state_file, &config.skills.state_file)?;
            }
        }
        config.subagent.enabled = eval.agent.enable_subagents;
        config.subagent.persist_sessions = eval.agent.enable_subagents;
        // Reject implicit live connector-store reloads. Each run uses the explicit config snapshot.
        if !eval.agent.enable_mcp {
            config.mcp.servers.clear();
        }
        let store = Arc::new(SessionStore::new(&config.session.directory).await?);
        let attachments = Arc::new(AttachmentStore::new(private.join("attachments")));
        let processes = Arc::new(ProcessManager::default());
        phases_ms.insert("storage".into(), started.elapsed().as_secs_f64() * 1000.0);
        let started = Instant::now();
        let mut approval = config.approval.clone();
        approval.policy = ApprovalPolicy::Never;
        for name in ["save_skill", "install_skill", "memory"] {
            approval.rules.insert(name.into(), ToolRule::Deny);
        }
        let approval_context = ApprovalContext {
            manager: Arc::new(ApprovalManager::new(std::time::Duration::from_secs(1))),
            policy: PermissionSnapshot {
                mode: PermissionMode::WorkspaceAccess,
                revision: "eval-fixed-v1".into(),
                approval: Some(approval),
            },
            session_id: uuid::Uuid::new_v4().to_string(),
            run_id: uuid::Uuid::new_v4().to_string(),
        };
        let agent = Agent::from_config_with_store(&config, store.clone())
            .await?
            .with_mcp_manager(Arc::new(McpManager::new(config.mcp.clone())?))
            .with_approvals(approval_context)
            .with_attachments(attachments.clone())
            .with_processes(processes.clone())
            .with_environment(environment)
            .bind_model(None)?;
        phases_ms.insert(
            "agent_construction".into(),
            started.elapsed().as_secs_f64() * 1000.0,
        );
        Ok(Self {
            agent,
            attachments,
            processes,
            store,
            workspace,
            phases_ms,
        })
    }
    pub async fn shutdown(&self) {
        self.processes.shutdown();
        self.agent.shutdown().await;
    }
}
pub fn load_app(path: Option<&Path>) -> Result<AppConfig> {
    path.map_or_else(kyler_config::load, kyler_config::load_from)
}
pub fn public_configuration(app: &AppConfig, eval: &EvalConfig) -> Result<Value> {
    let models = app.effective_models();
    let profiles: BTreeMap<_,_> = models.profiles.iter().map(|(id, p)| (id, json!({"model":p.model,"provider":p.provider,"context_tokens":p.context_tokens,"output_tokens":p.output_tokens,"capabilities":p.capabilities}))).collect();
    let providers: BTreeMap<_, _> = models
        .providers
        .iter()
        .map(|(id, p)| {
            let endpoint = reqwest::Url::parse(&p.base_url).ok().map(|mut u| {
                let _ = u.set_username("");
                let _ = u.set_password(None);
                u.set_query(None);
                u.set_fragment(None);
                u.to_string()
            });
            (id, endpoint)
        })
        .collect();
    let connectors: BTreeMap<_,_> = app.mcp.servers.iter().map(|(id,s)| (id, json!({"enabled":s.enabled,"auto_connect":s.auto_connect,"transport":s.transport,"activity":s.activity}))).collect();
    let skills = if eval.agent.load_user_skills {
        Some(skill_tree(&app.skills.user_directory, None)?)
    } else {
        None
    };
    Ok(
        json!({"skills_content_fingerprint":skills,"skills_state_fingerprint":if eval.agent.load_user_skills && app.skills.state_file.is_file(){Some(crate::gaia::dataset::digest_file(&app.skills.state_file)?)}else{None},"eval":eval,"default_model":models.default,"profiles":profiles,"providers":providers,"connectors":connectors,"connector_definition_fingerprint":crate::report::fingerprint(&app.mcp).ok(),"sandbox":{"mode":"workspace-write","network":app.sandbox.network_access,"commands":app.sandbox.allow_commands},"memory":false,"approval":"never; fixed sandbox ceiling; no human interaction","interactive_connector_store":false,"application_instructions_fingerprint":crate::report::fingerprint(&app.agent.additional_instructions).ok(),"system_prompt_fingerprint":crate::report::fingerprint(&kyler_protocol::prompts::AGENT).ok()}),
    )
}

/// Snapshot opt-in skills into the private run storage; reject links and oversized trees.
pub(crate) fn skill_tree(source: &Path, destination: Option<&Path>) -> Result<String> {
    fn walk(
        root: &Path,
        path: &Path,
        destination: Option<&Path>,
        entries: &mut BTreeMap<PathBuf, String>,
        bytes: &mut u64,
    ) -> Result<()> {
        let metadata = std::fs::symlink_metadata(path)?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "Skill snapshots cannot contain symlinks"
        );
        let relative = path.strip_prefix(root)?;
        if metadata.is_dir() {
            if let Some(destination) = destination {
                std::fs::create_dir_all(destination.join(relative))?;
            }
            for entry in std::fs::read_dir(path)? {
                walk(root, &entry?.path(), destination, entries, bytes)?;
            }
        } else if metadata.is_file() {
            *bytes += metadata.len();
            anyhow::ensure!(
                *bytes <= 64 * 1024 * 1024 && entries.len() < 10000,
                "Skill snapshot exceeds limits"
            );
            entries.insert(
                relative.to_owned(),
                crate::gaia::dataset::digest_file(path)?,
            );
            if let Some(destination) = destination {
                std::fs::copy(path, destination.join(relative))?;
                anyhow::ensure!(
                    crate::gaia::dataset::digest_file(&destination.join(relative))?
                        == entries[relative],
                    "Skill changed while snapshotting"
                );
            }
        } else {
            anyhow::bail!("Unsupported skill snapshot file");
        }
        Ok(())
    }
    let mut entries = BTreeMap::new();
    if source.exists() {
        walk(source, source, destination, &mut entries, &mut 0u64)?;
    }
    crate::report::fingerprint(&entries)
}
