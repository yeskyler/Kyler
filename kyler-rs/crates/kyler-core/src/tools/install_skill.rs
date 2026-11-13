//! Import validated packages without running repository or skill scripts.
use crate::{
    approval::ToolOperation,
    skills::SkillManager,
    tools::{ActivityKind, Tool, ToolContext, ToolExecutionMode},
};

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Result, ensure};
use serde_json::{Value, json};

use kyler_protocol::api::skills::{InstallSkillRequest, SkillScope};

pub struct InstallSkillTool {
    pub manager: Arc<SkillManager>,
    pub workspace: Option<PathBuf>,
}

pub(super) fn check_access(
    context: &ToolContext<'_>,
    manager: &SkillManager,
    workspace: Option<&Path>,
    scope: SkillScope,
    name: &str,
) -> Result<()> {
    if let Some(environment) = context.environment {
        ensure!(
            !environment.policy().filesystem.readwrite_paths.is_empty(),
            "sandbox is read-only; skill installation is blocked"
        );
        if !name.is_empty() {
            manager.check_destination_policy(
                workspace,
                scope,
                name,
                &environment.policy().filesystem,
            )?;
        }
    }
    Ok(())
}

#[async_trait::async_trait]
impl Tool for InstallSkillTool {
    fn name(&self) -> &str {
        "install_skill"
    }
    fn description(&self) -> &str {
        "Install a local skill directory containing SKILL.md into configured user (default) or project skills. Copies validated resources including binary assets, rejects symlinks, and never executes scripts. To install from Git, fetch the user-selected repository with exec_command first. Updates require the installed expected_fingerprint."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"scope":{"type":"string","enum":["user","project"]},"expected_fingerprint":{"type":["string","null"]}},"required":["path"],"additionalProperties":false})
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Host
    }
    fn activity_kind(&self) -> ActivityKind {
        ActivityKind::Edit
    }
    fn approval_requirement(&self, _: &Value) -> Result<ToolOperation> {
        Ok(ToolOperation::Write)
    }
    fn approval_identity(&self) -> String {
        format!(
            "{}:{:?}:{:?}",
            self.name(),
            self.manager.settings.user_directory,
            self.workspace
        )
    }
    async fn execute(&self, args: &str, context: &mut ToolContext<'_>) -> Result<String> {
        let request: InstallSkillRequest = serde_json::from_str(args)?;
        if let Some(environment) = context.environment {
            environment.policy().filesystem.check_read(
                &environment.session_environment().working_directory,
                &request.path,
            )?;
        }
        // The loader determines the actual destination name before publication.
        super::install_skill::check_access(
            context,
            &self.manager,
            self.workspace.as_deref(),
            request.scope,
            "",
        )?;
        Ok(serde_json::to_string(
            &self
                .manager
                .install_checked(
                    self.workspace.as_deref(),
                    request,
                    context.environment.map(|e| e.policy().filesystem.clone()),
                )
                .await?,
        )?)
    }
}
