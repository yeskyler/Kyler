//! Agent configuration and public execution entry points.
mod authorization;
mod builder;
mod delegation;
pub mod events;
mod execution;
mod execution_loop;
mod model_binding;
mod model_context;
mod model_stream;
mod recording;
mod skill_context;
#[cfg(test)]
mod tests;
mod tool_dispatch;
mod tool_snapshot;
mod turn;

use crate::{
    approval::ApprovalContext,
    attachments::{AttachmentStore, image::UserImage},
    context::{ContextPolicy, ExecutionContext},
    environment::{ExecutionEnvironment, SessionEnvironment},
    mcp::McpManager,
    memory::MemoryStore,
    models::{ModelRegistry, ModelSnapshot},
    processes::ProcessManager,
    session::{Session, store::SessionStore},
    skills::{SkillManager, SkillSnapshot},
    subagent::{SubagentExecutor, SubagentManager},
    tools::{ToolBox, ToolOutputBudget},
};

use std::sync::Arc;

use anyhow::Result;

use kyler_protocol::{
    api::images::ImageInput,
    prompts::{self, PromptRole},
};

pub use events::{AgentCancelHandle, AgentStream, AgentStreamEvent, AgentStreamOutcome};
pub use turn::UserTurn;
pub mod observation;

/// Compatibility path for stream event types.
pub use events as stream;

#[derive(Debug)]
pub struct AgentResult {
    pub output: String,
    pub context: ExecutionContext,
    /// Snapshot; the mutable Session passed to run_in_session remains authoritative.
    pub session: Session,
}
#[derive(Clone)]
pub struct Agent {
    observer: Option<Arc<dyn observation::RuntimeObserver>>,
    process_manager: Option<Arc<ProcessManager>>,
    attachments: Option<Arc<AttachmentStore>>,
    memory_store: Option<Arc<MemoryStore>>,
    approval: Option<ApprovalContext>,
    model_registry: Option<Arc<ModelRegistry>>,
    model_snapshot: Option<Arc<ModelSnapshot>>,
    base_url: String,
    api_key: String,
    model: String,
    additional_instructions: Option<String>,
    prompt_role: PromptRole,
    toolbox: Arc<ToolBox>,
    mcp: Option<Arc<McpManager>>,
    mcp_scope: Option<String>,
    skills: Option<Arc<SkillManager>>,
    skill_snapshot: Option<Arc<SkillSnapshot>>,
    max_steps: u32,
    context_policy: ContextPolicy,
    tool_output_budget: ToolOutputBudget,
    session_store: Option<Arc<SessionStore>>,
    subagent_manager: Option<Arc<SubagentManager>>,
    environment: Option<Arc<ExecutionEnvironment>>,
}
impl Agent {
    pub async fn run_turn_in_session(
        &self,
        session: Session,
        turn: UserTurn,
    ) -> Result<AgentStream> {
        self.tool_output_budget.validate()?;
        Ok(AgentStream::start_turn(
            self.bind_model(None)?,
            session,
            turn,
        ))
    }

    pub fn skill_manager(&self) -> Option<&Arc<SkillManager>> {
        self.skills.as_ref()
    }

    pub fn mcp_manager(&self) -> Option<&Arc<McpManager>> {
        self.mcp.as_ref()
    }
    /// Latest session permission choice applied to the immutable host boundary.
    pub fn effective_execution_environment(&self) -> Option<Arc<ExecutionEnvironment>> {
        self.permission_environment().1
    }

    pub fn subagent_manager(&self) -> Option<&Arc<SubagentManager>> {
        self.subagent_manager.as_ref()
    }

    pub fn execution_environment(&self) -> Option<Arc<ExecutionEnvironment>> {
        self.environment.clone()
    }

    pub fn workspace_environment(&self) -> Option<&SessionEnvironment> {
        self.environment
            .as_ref()
            .map(|environment| environment.session_environment())
    }
    fn system_prompt(&self) -> String {
        let mut prompt =
            prompts::agent_prompt(self.additional_instructions.as_deref(), self.prompt_role);
        if let Some(snapshot) = &self.skill_snapshot {
            let catalog = snapshot.catalog_text();
            if !catalog.is_empty() {
                prompt.push_str("\n\n");
                prompt.push_str(&catalog);
            }
        }
        prompt
    }

    pub async fn run_stream_with_images_in_session(
        &self,
        session: Session,
        input: &str,
        images: Vec<ImageInput>,
    ) -> Result<AgentStream> {
        let prepared =
            tokio::task::spawn_blocking(move || crate::attachments::image::prepare_images(&images))
                .await??;
        self.run_stream_with_prepared_images_in_session(session, input, prepared)
            .await
    }

    /// Uses images already validated by prepare_images (for server admission).
    pub async fn run_stream_with_prepared_images_in_session(
        &self,
        session: Session,
        input: &str,
        images: Vec<UserImage>,
    ) -> Result<AgentStream> {
        anyhow::ensure!(
            images.len() <= kyler_protocol::api::images::MAX_IMAGES,
            "Too many images"
        );
        self.tool_output_budget.validate()?;
        Ok(AgentStream::start_with_images(
            self.bind_model(None)?,
            session,
            input.to_owned(),
            images,
        ))
    }

    /// Starts an owned background execution; dropping the handle requests cancellation.
    pub async fn run_stream(&self, input: &str) -> Result<AgentStream> {
        self.run_stream_in_session(Session::new(), input).await
    }

    /// Takes ownership of the session. Use finish_with_session to recover it on errors too.
    pub async fn run_stream_in_session(
        &self,
        session: Session,
        input: &str,
    ) -> Result<AgentStream> {
        self.tool_output_budget.validate()?;
        Ok(AgentStream::start(
            self.bind_model(None)?,
            session,
            input.to_owned(),
        ))
    }

    pub async fn run(&self, input: &str) -> Result<AgentResult> {
        self.run_in_session(&mut Session::new(), input).await
    }
    pub async fn run_in_session(&self, session: &mut Session, input: &str) -> Result<AgentResult> {
        let output = self.execute_session(session, input, None).await?;
        Ok(AgentResult {
            output,
            context: session.current_execution()?.clone(),
            session: session.clone(),
        })
    }
}

#[async_trait::async_trait]
impl SubagentExecutor for Agent {
    fn for_task(
        &self,
        request: &crate::subagent::DelegateTaskArgs,
    ) -> Result<Option<Arc<dyn SubagentExecutor>>> {
        match request.agent {
            crate::subagent::AgentKind::General => Ok(None),
            crate::subagent::AgentKind::Review => Ok(Some(Arc::new(
                crate::subagent::review::ReviewExecutor::new(
                    self.clone(),
                    request
                        .review
                        .clone()
                        .ok_or_else(|| anyhow::anyhow!("Missing review target"))?,
                )?,
            ))),
        }
    }

    fn waiting_for_approval(&self, session: &str) -> bool {
        self.approval
            .as_ref()
            .is_some_and(|a| a.manager.waiting_for_session(session))
    }
    fn session_environment(&self) -> Option<SessionEnvironment> {
        self.environment
            .as_ref()
            .map(|e| e.session_environment().clone())
    }
    async fn execute(&self, session: &mut Session, input: &str) -> Result<String> {
        Ok(self.run_in_session(session, input).await?.output)
    }
}
