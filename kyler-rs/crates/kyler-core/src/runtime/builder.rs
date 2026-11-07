//! Explicit application-to-runtime adapter. No file loading or global subscriber installation.
use super::Agent;
use crate::{
    approval::ApprovalContext,
    attachments::AttachmentStore,
    context::ContextPolicy,
    environment::ExecutionEnvironment,
    mcp::McpManager,
    memory::{MemoryStore, MemoryTool},
    models::ModelRegistry,
    processes::{ProcessManager, ProcessTool},
    session::{Session, SessionStore},
    skills::SkillManager,
    subagent::{SubagentConfig, SubagentManager},
    tools::{
        Tool, ToolBox, ToolOutputBudget, attachment::AttachmentTool, build_toolbox,
        command::RunCommandTool, subagent_tools,
    },
};

use std::sync::Arc;

use anyhow::Result;

use kyler_config::{AgentSettings, AppConfig};
use kyler_protocol::prompts::PromptRole;

impl Agent {
    pub async fn from_config(config: &AppConfig) -> Result<Self> {
        config.validate()?;
        let toolbox = Arc::new(build_toolbox().await?);
        Self::from_config_with_tools(config, toolbox).await
    }

    /// Custom toolboxes are explicit replacements; tools.* does not rewrite their entries.
    pub async fn from_config_with_tools(config: &AppConfig, toolbox: Arc<ToolBox>) -> Result<Self> {
        Self::configured(config, toolbox, None).await
    }

    pub async fn from_config_with_store(
        config: &AppConfig,
        store: Arc<SessionStore>,
    ) -> Result<Self> {
        let toolbox = Arc::new(build_toolbox().await?);
        Self::configured(config, toolbox, Some(store)).await
    }

    pub async fn from_config_with_tools_and_store(
        config: &AppConfig,
        toolbox: Arc<ToolBox>,
        store: Arc<SessionStore>,
    ) -> Result<Self> {
        Self::configured(config, toolbox, Some(store)).await
    }

    async fn configured(
        config: &AppConfig,
        toolbox: Arc<ToolBox>,
        store: Option<Arc<SessionStore>>,
    ) -> Result<Self> {
        config.validate()?;
        let policy = ContextPolicy::from(&config.context);
        policy.validate()?;
        let mut agent = Self::new(
            config.llm.base_url.clone(),
            config.llm.api_key.expose().to_owned(),
            config.llm.model.clone(),
            config.agent.additional_instructions.clone(),
            toolbox,
        )
        .with_max_steps(config.agent.max_steps)
        .with_context_policy(policy)
        .with_tool_output_budget(ToolOutputBudget {
            max_bytes: config.agent.tool_output_max_bytes,
        });
        let registry = Arc::new(ModelRegistry::new(config)?);
        agent.model_registry = Some(registry);
        let bound = agent.bind_model(None)?;
        agent.base_url = bound.base_url;
        agent.api_key = bound.api_key;
        agent.model = bound.model;
        agent.context_policy = bound.context_policy;
        if config.skills.enabled {
            agent.skills = Some(Arc::new(SkillManager::new(config.skills.clone())?));
        }
        agent.mcp = Some(Arc::new(
            McpManager::new(config.mcp.clone())?
                .with_user_store(config.session.directory.join("_connectors.json"))?,
        ));
        if let Some(store) = store {
            agent = agent.with_session_store(store);
        } else if config.session.enabled {
            agent = agent.with_session_store(Arc::new(
                SessionStore::new(&config.session.directory).await?,
            ));
        }
        if config.subagent.enabled {
            let settings = SubagentConfig::from(&config.subagent);
            let mut manager = SubagentManager::new(settings)?;
            if config.subagent.persist_sessions {
                manager = manager.with_session_store(
                    agent
                        .session_store
                        .as_ref()
                        .expect("validated session storage")
                        .clone(),
                );
            }
            agent = agent.with_subagent_manager(Arc::new(manager));
        }
        Ok(agent)
    }

    /// Stop outstanding child work before shutting down the application runtime.
    pub async fn shutdown(&self) {
        if let Some(manager) = &self.subagent_manager {
            manager.shutdown().await;
        }
        if let Some(manager) = &self.mcp {
            manager.shutdown().await;
        }
    }
}

impl Agent {
    /// Creates Kyler with the shared default prompt and optional application instructions.
    /// None uses the default prompt; Some supplements it rather than replacing it.
    pub fn new(
        base_url: String,
        api_key: String,
        model: impl Into<String>,
        additional_instructions: Option<impl Into<String>>,
        toolbox: Arc<ToolBox>,
    ) -> Self {
        Self {
            observer: None,
            process_manager: None,
            attachments: None,
            memory_store: None,
            approval: None,
            model_registry: None,
            model_snapshot: None,
            base_url,
            api_key,
            model: model.into(),
            additional_instructions: additional_instructions.map(Into::into),
            prompt_role: PromptRole::Assistant,
            toolbox,
            mcp: None,
            mcp_scope: None,
            skills: None,
            skill_snapshot: None,
            max_steps: AgentSettings::default().max_steps,
            context_policy: ContextPolicy::default(),
            tool_output_budget: ToolOutputBudget::default(),
            session_store: None,
            subagent_manager: None,
            environment: None,
        }
    }
    pub fn with_questions(
        mut self,
        manager: Arc<crate::questions::QuestionManager>,
        waiter: Option<Arc<dyn crate::questions::QuestionWaiter>>,
    ) -> Self {
        Arc::make_mut(&mut self.toolbox).insert(
            "ask_question".into(),
            Arc::new(crate::tools::ask_question::AskQuestionTool { manager, waiter }),
        );
        self
    }
    pub fn with_processes(mut self, manager: Arc<ProcessManager>) -> Self {
        self.process_manager = Some(manager.clone());
        for action in ["read_process", "write_process", "stop_process"] {
            Arc::make_mut(&mut self.toolbox).insert(
                action.into(),
                Arc::new(ProcessTool {
                    manager: manager.clone(),
                    action,
                }),
            );
        }
        self
    }
    pub fn with_project_memory(mut self, store: Arc<MemoryStore>, session: &Session) -> Self {
        let tool = MemoryTool {
            store: store.clone(),
            scope: MemoryStore::scope(session),
        };
        Arc::make_mut(&mut self.toolbox).insert("memory".into(), Arc::new(tool));
        self.memory_store = Some(store);
        self
    }
    pub fn with_attachments(mut self, store: Arc<AttachmentStore>) -> Self {
        let tools = Arc::make_mut(&mut self.toolbox);
        for inspect in [true, false] {
            let tool = AttachmentTool {
                store: store.clone(),
                inspect,
            };
            tools.insert(tool.name().into(), Arc::new(tool));
        }
        self.attachments = Some(store);
        self
    }
    pub fn with_max_steps(mut self, max_steps: u32) -> Self {
        self.max_steps = max_steps;
        self
    }
    /// Optional payload-free measurements; does not alter events or execution policy.
    pub fn with_observer(mut self, observer: Arc<dyn super::observation::RuntimeObserver>) -> Self {
        self.observer = Some(observer);
        self
    }
    pub fn with_context_policy(mut self, policy: ContextPolicy) -> Self {
        self.context_policy = policy;
        self
    }
    pub fn with_tool_output_budget(mut self, budget: ToolOutputBudget) -> Self {
        self.tool_output_budget = budget;
        self
    }
    /// Bind a trusted immutable sandbox policy. A persisted policy ID alone never grants access.
    pub fn with_environment(mut self, environment: Arc<ExecutionEnvironment>) -> Self {
        if environment.policy().process.allow_commands {
            if self.process_manager.is_none() {
                self = self.with_processes(Arc::new(ProcessManager::default()));
            }
            let mut tools = self.toolbox.as_ref().clone();
            tools.insert("run_command".into(), Arc::new(RunCommandTool));
            self.toolbox = Arc::new(tools);
        }
        self.environment = Some(environment);
        self
    }
    /// Enable automatic checkpoints after input, compaction, tool results and completion.
    pub fn with_session_store(mut self, store: Arc<SessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }
    /// Enables background delegation and registers its four management tools.
    pub fn with_subagent_manager(mut self, manager: Arc<SubagentManager>) -> Self {
        let mut tools = self.toolbox.as_ref().clone();
        for tool in subagent_tools() {
            tools.insert(tool.name().into(), tool);
        }
        self.toolbox = Arc::new(tools);
        self.subagent_manager = Some(manager);
        self
    }
    pub fn with_mcp_manager(mut self, manager: Arc<McpManager>) -> Self {
        self.mcp = Some(manager);
        self
    }
    pub fn with_skill_manager(mut self, manager: Arc<SkillManager>) -> Self {
        self.skills = Some(manager);
        self
    }
    pub fn with_mcp_scope(mut self, scope: impl Into<String>) -> Self {
        self.mcp_scope = Some(scope.into());
        self
    }
    pub fn with_approvals(mut self, context: ApprovalContext) -> Self {
        self.approval = Some(context);
        self
    }
}
