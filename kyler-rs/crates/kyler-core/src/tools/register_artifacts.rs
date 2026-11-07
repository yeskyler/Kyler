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
    files: Vec<Deliverable>,
}

pub struct RegisterArtifactsTool;

#[async_trait::async_trait]
impl Tool for RegisterArtifactsTool {
    fn name(&self) -> &str {
        "register_artifacts"
    }
    fn description(&self) -> &str {
        "Replace this execution's final deliverable list. Use local workspace-relative URL-encoded paths, display names including the actual extensions, and at most one primary file for automatic preview. Only register completed deliverables, never intermediate files or secrets. An empty list clears registration. Files are validated now and retained by the host at task completion."
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
        validate_deliverables(&args.files)?;
        if !args.files.is_empty() {
            let environment = context.environment.context("no authorized workspace")?;
            for file in &args.files {
                read_source(&file.path, environment).map_err(|_| anyhow::anyhow!("Deliverable unavailable: check its path, permissions, and size (20 MiB maximum)."))?;
            }
        }
        let count = args.files.len();
        context.execution.deliverables = args.files;
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
