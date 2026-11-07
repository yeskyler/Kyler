//! Construct child runtimes with inherited capabilities and isolated sessions.
use super::Agent;
use crate::{
    subagent::{SubagentExecutor, SubagentManager},
    tools::is_subagent_management,
};

use std::sync::Arc;

use kyler_protocol::prompts::PromptRole;

impl Agent {
    pub(crate) fn review_runtime(
        &self,
        environment: Arc<crate::ExecutionEnvironment>,
        tools: crate::ToolBox,
        repair: bool,
    ) -> Self {
        let instructions = repair.then(|| "You are repairing only the format of the preceding review report. Do not add claims, execute tools, or start a new review. Return only valid JSON matching the requested report schema.".to_owned());
        let mut child = Self::new(
            self.base_url.clone(),
            self.api_key.clone(),
            self.model.clone(),
            instructions,
            Arc::new(tools),
        )
        .with_max_steps(if repair { 1 } else { self.max_steps })
        .with_context_policy(self.context_policy.clone())
        .with_tool_output_budget(self.tool_output_budget);
        child.environment = Some(environment);
        child.model_snapshot = self.model_snapshot.clone();
        child.session_store = self.session_store.clone();
        child.observer = self.observer.clone();
        child.prompt_role = PromptRole::Review;
        child
    }
    pub(super) fn child_executor(&self, manager: &SubagentManager) -> Arc<dyn SubagentExecutor> {
        let tools = self
            .toolbox
            .iter()
            .filter(|(_, tool)| {
                !is_subagent_management(tool.as_ref()) && tool.name() != "ask_question"
            })
            .map(|(name, tool)| (name.clone(), Arc::clone(tool)))
            .collect();
        let mut child = Self::new(
            self.base_url.clone(),
            self.api_key.clone(),
            self.model.clone(),
            self.additional_instructions.clone(),
            Arc::new(tools),
        )
        .with_max_steps(manager.config().max_steps)
        .with_context_policy(self.context_policy.clone())
        .with_tool_output_budget(self.tool_output_budget);
        child.process_manager = self.process_manager.clone();
        child.attachments = self.attachments.clone();
        child.memory_store = self.memory_store.clone();
        child.approval = self.approval.clone();
        child.model_snapshot = self.model_snapshot.clone();
        child.observer = self.observer.clone();
        child.prompt_role = PromptRole::Subagent;
        child.session_store = manager.child_store();
        child.environment = self.environment.clone();
        child.mcp = self.mcp.clone();
        child.skills = self.skills.clone();
        child.mcp_scope = self.mcp_scope.clone();
        Arc::new(child)
    }
}
