//! Bind session-specific MCP tools and reviewed skills to an execution.
use super::Agent;
use crate::{
    sandbox::SandboxMode,
    tools::{
        ToolBox, ToolExecutionMode, install_skill::InstallSkillTool, load_skill::LoadSkillTool,
        save_skill::SaveSkillTool, skills_resource::SkillsResourceTool,
    },
};

use std::sync::Arc;

use anyhow::Result;

use kyler_protocol::api::permissions::PermissionMode;

impl Agent {
    pub(super) async fn tool_snapshot(&self, session: &str) -> Result<Self> {
        let mut agent = self.clone();
        if self.prompt_role == kyler_protocol::prompts::PromptRole::Review {
            anyhow::ensure!(
                self.environment
                    .as_ref()
                    .is_some_and(|environment| environment.policy().mode == SandboxMode::ReadOnly),
                "Review requires a read-only environment"
            );
            anyhow::ensure!(
                self.toolbox.keys().all(|name| matches!(
                    name.as_str(),
                    "read_review_file" | "list_review_files" | "search_review_files"
                )),
                "Review tool registry contains a forbidden capability"
            );
            return Ok(agent);
        }
        if let Some(context) = &mut agent.approval {
            context.policy = context.current_policy();
        }
        let read_only = agent
            .approval
            .as_ref()
            .is_some_and(|context| context.policy.mode == PermissionMode::ReadOnly)
            || agent
                .environment
                .as_ref()
                .is_some_and(|e| e.policy().mode == SandboxMode::ReadOnly);
        let (_, effective_environment) = agent.permission_environment();
        let mcp_allowed = !read_only
            && agent.environment.as_ref().map(|e| e.policy().mode)
                == effective_environment.as_ref().map(|e| e.policy().mode);
        if !mcp_allowed {
            Arc::make_mut(&mut agent.toolbox)
                .retain(|_, tool| tool.execution_mode() != ToolExecutionMode::Mcp);
        }
        if let Some(manager) = &self.mcp
            && mcp_allowed
        {
            let started = std::time::Instant::now();
            let tools = manager
                .snapshot(
                    self.mcp_scope.as_deref().unwrap_or(session),
                    self.environment.clone(),
                )
                .await?;
            agent.toolbox = Arc::new(merge_tools(&self.toolbox, tools)?);
            self.observe(super::observation::RuntimeObservation::McpSnapshot {
                elapsed: started.elapsed(),
            });
        }
        if let Some(manager) = &self.skills {
            let started = std::time::Instant::now();
            let workspace = self
                .environment
                .as_ref()
                .map(|e| e.session_environment().workspace_root.as_path());
            let snapshot = Arc::new(manager.snapshot(workspace).await?);
            self.observe(super::observation::RuntimeObservation::SkillSnapshot {
                elapsed: started.elapsed(),
            });
            let mut tools = agent.toolbox.as_ref().clone();
            if manager.settings.enabled && !read_only {
                for name in ["save_skill", "install_skill"] {
                    anyhow::ensure!(
                        !tools.contains_key(name),
                        "reserved skill tool name conflicts with a configured tool"
                    );
                }
                tools.insert(
                    "save_skill".into(),
                    Arc::new(SaveSkillTool {
                        manager: manager.clone(),
                        workspace: workspace.map(ToOwned::to_owned),
                    }),
                );
                tools.insert(
                    "install_skill".into(),
                    Arc::new(InstallSkillTool {
                        manager: manager.clone(),
                        workspace: workspace.map(ToOwned::to_owned),
                    }),
                );
            }
            if !snapshot.available.is_empty() {
                anyhow::ensure!(
                    !tools.contains_key("load_skill") && !tools.contains_key("skills_resource"),
                    "reserved skill tool name conflicts with a configured tool"
                );
                tools.insert(
                    "load_skill".into(),
                    Arc::new(LoadSkillTool(snapshot.clone())),
                );
                tools.insert(
                    "skills_resource".into(),
                    Arc::new(SkillsResourceTool(snapshot.clone())),
                );
            }
            agent.toolbox = Arc::new(tools);
            agent.skill_snapshot = Some(snapshot);
        }
        Ok(agent)
    }
}

/// Freeze local and remote tools together without allowing remote name replacement.
fn merge_tools(builtins: &ToolBox, remote: ToolBox) -> Result<ToolBox> {
    let mut tools = builtins.clone();
    for (name, tool) in remote {
        anyhow::ensure!(
            !tools.contains_key(&name),
            "MCP tool name collides with an existing tool"
        );
        tools.insert(name, tool);
    }
    Ok(tools)
}
