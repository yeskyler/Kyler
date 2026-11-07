//! Persist a complete skill package through the approved application control plane.
use crate::{
    approval::ToolOperation,
    skills::SkillManager,
    tools::{ActivityKind, Tool, ToolContext, ToolExecutionMode},
};

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use serde_json::{Value, json};

use kyler_protocol::api::skills::{SaveSkillRequest, SkillScope};

pub struct SaveSkillTool {
    pub manager: Arc<SkillManager>,
    pub workspace: Option<PathBuf>,
}
#[async_trait::async_trait]
impl Tool for SaveSkillTool {
    fn name(&self) -> &str {
        "save_skill"
    }
    fn description(&self) -> &str {
        "Create or update a complete UTF-8 skill package in the configured user or project skill directory. Include SKILL.md in files. Defaults to user scope. Updates require the existing expected_fingerprint. Built-ins are customized via a user override. Validates before publishing and refreshes discovery; project skills still require review in /skills. Does not execute scripts."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"name":{"type":"string"},"scope":{"type":"string","enum":["user","project"]},"files":{"type":"object","additionalProperties":{"type":"string"}},"expected_fingerprint":{"type":["string","null"]}},"required":["name","files"],"additionalProperties":false})
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
        let request: SaveSkillRequest = serde_json::from_str(args)?;
        super::install_skill::check_access(
            context,
            &self.manager,
            self.workspace.as_deref(),
            request.scope,
            &request.name,
        )?;
        if request.scope == SkillScope::Project {
            if let Some(environment) = context.environment {
                let root = self
                    .manager
                    .destination(self.workspace.as_deref(), request.scope)?
                    .join(&request.name);
                for path in request.files.keys() {
                    environment
                        .policy()
                        .filesystem
                        .check_write(&root, &root.join(path))?;
                }
            }
        }
        Ok(serde_json::to_string(
            &self
                .manager
                .save(self.workspace.as_deref(), request)
                .await?,
        )?)
    }
}
