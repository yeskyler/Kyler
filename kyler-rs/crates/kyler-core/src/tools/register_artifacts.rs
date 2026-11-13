use crate::{
    approval::ToolOperation,
    artifacts::{Deliverable, read_source, validate_deliverables},
    tools::{Tool, ToolContext, ToolExecutionMode},
};

use anyhow::Context;
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, JsonSchema)]
struct RegisterArtifactsArgs {
    files: Vec<ArtifactInput>,
}

#[derive(Deserialize, JsonSchema)]
struct ArtifactInput {
    path: String,
    /// Defaults to the source filename. A missing extension is appended from the source.
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    primary: bool,
}

impl ArtifactInput {
    fn into_deliverable(self) -> anyhow::Result<Deliverable> {
        let decoded = percent_encoding::percent_decode_str(
            self.path.split(['?', '#']).next().unwrap_or(&self.path),
        )
        .decode_utf8()?;
        let source = std::path::Path::new(decoded.as_ref());
        let mut name = match self.name {
            Some(name) => name,
            None => source
                .file_name()
                .and_then(|name| name.to_str())
                .context("invalid deliverable path")?
                .to_owned(),
        };
        if !name.trim().is_empty() && std::path::Path::new(&name).extension().is_none() {
            if let Some(extension) = source.extension().and_then(|extension| extension.to_str()) {
                name.push('.');
                name.push_str(extension);
            }
        }
        Ok(Deliverable {
            path: self.path,
            name,
            primary: self.primary,
        })
    }
}

pub struct RegisterArtifactsTool;

#[async_trait::async_trait]
impl Tool for RegisterArtifactsTool {
    fn name(&self) -> &str {
        "register_artifacts"
    }
    fn description(&self) -> &str {
        "Replace this execution's final deliverable list. Use local workspace-relative URL-encoded paths, optional display names (default source filename; missing source extensions are appended, conflicting extensions are rejected), and at most one primary file for automatic preview. Only register completed deliverables, never intermediate files or secrets. An empty list clears registration. Files are validated now and retained by the host at task completion."
    }
    fn parameters(&self) -> Value {
        serde_json::to_value(schema_for!(RegisterArtifactsArgs)).expect("valid artifact schema")
    }
    fn approval_requirement(&self, _: &Value) -> anyhow::Result<ToolOperation> {
        Ok(ToolOperation::State)
    }
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Host
    }
    async fn execute(
        &self,
        args_json: &str,
        context: &mut ToolContext<'_>,
    ) -> anyhow::Result<String> {
        let args: RegisterArtifactsArgs =
            serde_json::from_str(args_json).context("invalid artifact registration")?;
        let files = args
            .files
            .into_iter()
            .map(ArtifactInput::into_deliverable)
            .collect::<anyhow::Result<Vec<_>>>()?;
        validate_deliverables(&files)?;
        if !files.is_empty() {
            let environment = context.environment.context("no authorized workspace")?;
            for file in &files {
                read_source(&file.path, environment).map_err(|_| anyhow::anyhow!("Deliverable unavailable: check its path, permissions, and size (20 MiB maximum)."))?;
            }
        }
        let count = files.len();
        context.execution.deliverables = files;
        Ok(serde_json::json!({"registered": count, "retention": "pending_completion"}).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ExecutionEnvironment, SandboxPolicy, context::ExecutionContext,
        sandbox::direct::DirectSandbox,
    };
    use std::{collections::HashMap, sync::Arc};

    #[test]
    fn registration_names_are_derived_without_changing_persistent_contracts() {
        for (input, expected) in [
            (r#"{"path":"./snake_game.html"}"#, "snake_game.html"),
            (
                r#"{"path":"./snake_game.html","name":"贪吃蛇游戏"}"#,
                "贪吃蛇游戏.html",
            ),
            (
                r#"{"path":"./snake_game.html","name":"Game.HTML"}"#,
                "Game.HTML",
            ),
            (r#"{"path":"./my%20game.html"}"#, "my game.html"),
            (r#"{"path":"./LICENSE"}"#, "LICENSE"),
        ] {
            let file = serde_json::from_str::<ArtifactInput>(input)
                .unwrap()
                .into_deliverable()
                .unwrap();
            assert_eq!(file.name, expected);
            assert!(!file.primary);
            validate_deliverables(&[file]).unwrap();
        }
        for input in [
            r#"{"path":"./game.html","name":"Game.txt"}"#,
            r#"{"path":"./game.html","name":""}"#,
            r#"{"path":"./game.html","name":"../Game"}"#,
        ] {
            let file = serde_json::from_str::<ArtifactInput>(input)
                .unwrap()
                .into_deliverable()
                .unwrap();
            assert!(validate_deliverables(&[file]).is_err());
        }
    }

    #[tokio::test]
    async fn registration_validates_before_replacing_execution_state() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().canonicalize().unwrap();
        std::fs::write(workspace.join("final.txt"), "final").unwrap();
        let environment = ExecutionEnvironment::new(
            &workspace,
            &workspace,
            SandboxPolicy::workspace_write("artifacts", &workspace),
            std::env::current_exe().unwrap(),
            Arc::new(DirectSandbox),
        )
        .unwrap();
        let mut execution = ExecutionContext::new();
        let mut state = HashMap::new();
        let mut context = ToolContext {
            processes: None,
            execution: &mut execution,
            state: &mut state,
            session_id: "session",
            tool_call_id: "call",
            subagents: None,
            environment: Some(&environment),
        };
        let tool = RegisterArtifactsTool;
        tool.execute(
            r#"{"files":[{"path":"./final.txt","name":"Final.txt","primary":true}]}"#,
            &mut context,
        )
        .await
        .unwrap();
        assert!(context.execution.deliverables[0].primary);
        for (input, expected) in [
            (r#"{"files":[{"path":"./final.txt"}]}"#, "final.txt"),
            (
                r#"{"files":[{"path":"./final.txt","name":"Final"}]}"#,
                "Final.txt",
            ),
        ] {
            tool.execute(input, &mut context).await.unwrap();
            assert_eq!(context.execution.deliverables[0].name, expected);
        }
        assert!(
            tool.execute(
                r#"{"files":[{"path":"./final.txt","name":"Final.html"}]}"#,
                &mut context
            )
            .await
            .is_err()
        );
        assert_eq!(context.execution.deliverables[0].name, "Final.txt");
        for path in [
            "./missing.txt",
            "../outside.txt",
            "https://example.com/file.txt",
        ] {
            let args = serde_json::json!({"files":[{"path":path,"name":"Bad.txt"}]}).to_string();
            assert!(tool.execute(&args, &mut context).await.is_err());
            assert_eq!(context.execution.deliverables[0].name, "Final.txt");
        }
        tool.execute(r#"{"files":[]}"#, &mut context).await.unwrap();
        assert!(context.execution.deliverables.is_empty());
    }
}
