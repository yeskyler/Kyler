//! Authorize before announcing or executing a tool, preserving a durable decision record.
use super::Agent;
use crate::{
    approval::ToolOperation, environment::ExecutionEnvironment, sandbox::NetworkAccess,
    session::Session, tools::ToolExecutionMode,
};

use std::{path::Path, sync::Arc};

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};

use kyler_protocol::{
    api::permissions::{ApprovalRecord, ApprovalStatus, PermissionMode, PermissionSnapshot},
    approval::{ApprovalPolicy, ApprovalRequestKind, ApprovalReviewer, ToolRule},
};

impl Agent {
    /// Restrict each call from the immutable host ceiling using the latest
    /// session choice. Never derive a new call from a previously narrowed view.
    pub(super) fn permission_environment(
        &self,
    ) -> (
        Option<PermissionSnapshot>,
        Option<Arc<ExecutionEnvironment>>,
    ) {
        let policy = self
            .approval
            .as_ref()
            .map(|context| context.current_policy());
        let environment = self.environment.as_ref().map(|environment| {
            if policy
                .as_ref()
                .is_some_and(|policy| policy.mode == PermissionMode::ReadOnly)
            {
                Arc::new(environment.read_only())
            } else if policy
                .as_ref()
                .is_some_and(|policy| policy.mode.preset() == PermissionMode::WorkspaceAccess)
            {
                Arc::new(environment.workspace_access())
            } else {
                environment.clone()
            }
        });
        (policy, environment)
    }

    pub(super) async fn authorize_tool(
        &self,
        session: &mut Session,
        name: &str,
        call: &str,
        arguments: &Value,
    ) -> Result<Option<Arc<ExecutionEnvironment>>> {
        let tool = self
            .toolbox
            .get(name)
            .context("Unknown or unavailable tool.")?;
        let (clean_arguments, requested) = crate::approval::policy::arguments(arguments)?;
        tool.validate_arguments(&clean_arguments)?;
        let operation = tool.approval_requirement(&clean_arguments)?;
        if self.prompt_role == kyler_protocol::prompts::PromptRole::Review {
            ensure!(
                !requested
                    && operation == ToolOperation::Read
                    && matches!(
                        name,
                        "read_review_file" | "list_review_files" | "search_review_files"
                    ),
                "Review cannot execute mutable tools or request approval"
            );
            return Ok(self.environment.clone());
        }
        let Some(context) = &self.approval else {
            return Ok(self.environment.clone());
        };
        let mut changes = context
            .manager
            .policy_changes(&context.session_id, &context.policy);
        let policy = changes.borrow_and_update().clone();
        session.current_execution_mut()?.permissions = Some(policy.clone());
        let settings = policy.effective_approval();
        let (_, mut effective_environment) = self.permission_environment();
        let preflight = |environment: Option<&ExecutionEnvironment>| -> Result<()> {
            let arguments = &clean_arguments;
            if let Some(environment) = environment {
                let policy = environment.policy();
                ensure!(
                    tool.execution_mode() != ToolExecutionMode::Mcp
                        || self
                            .environment
                            .as_ref()
                            .is_some_and(|host| host.policy().mode == policy.mode),
                    "MCP connection belongs to a different sandbox boundary."
                );
                let cwd = &environment.session_environment().working_directory;
                ensure!(
                    tool.execution_mode() != ToolExecutionMode::Unspecified,
                    "Tool has no sandbox execution strategy."
                );
                if operation == ToolOperation::Command {
                    ensure!(policy.process.allow_commands, "Sandbox blocks commands.");
                }
                if operation == ToolOperation::Network {
                    ensure!(
                        !matches!(policy.network, NetworkAccess::Deny),
                        "Sandbox blocks network access."
                    );
                }
                match name {
                    "read_file" => {
                        policy.filesystem.check_read(
                            cwd,
                            Path::new(
                                arguments["file_path"]
                                    .as_str()
                                    .context("Missing file_path")?,
                            ),
                        )?;
                    }
                    "list_dir" => {
                        policy.filesystem.check_read(
                            cwd,
                            Path::new(arguments["dir_path"].as_str().context("Missing dir_path")?),
                        )?;
                    }
                    "grep_files" => {
                        policy.filesystem.check_read(
                            cwd,
                            Path::new(arguments["path"].as_str().unwrap_or(".")),
                        )?;
                    }
                    "apply_patch" => crate::tools::apply_patch::validate_patch_access(
                        &environment.session_environment().workspace_root,
                        arguments["input"].as_str().context("Missing patch input")?,
                        &policy.filesystem,
                    )?,
                    _ => {}
                }
            }
            Ok(())
        };
        let mut validation = preflight(effective_environment.as_deref());
        let mut escalation = false;
        if validation.is_err()
            && requested
            && tool.execution_mode().sandboxed()
            && !settings.rejects(ApprovalRequestKind::SandboxEscalation)
            && settings.rules.get(name) != Some(&ToolRule::Deny)
            && preflight(self.environment.as_deref()).is_ok()
        {
            effective_environment = self.environment.clone();
            escalation = true;
            validation = Ok(());
        }
        let mut status = if !escalation
            && policy.mode == PermissionMode::ReadOnly
            && !matches!(operation, ToolOperation::Read | ToolOperation::State)
        {
            ApprovalStatus::Denied
        } else {
            crate::approval::policy::decide(
                &settings,
                operation,
                &clean_arguments,
                tool.as_ref(),
                effective_environment.as_deref(),
                requested || escalation,
            )
        };
        let reason = if let Err(error) = &validation {
            status = ApprovalStatus::Denied;
            format!("Sandbox denied operation: {error}")
        } else if status == ApprovalStatus::Denied {
            if settings.rules.get(name) == Some(&ToolRule::Deny) {
                "Tool execution rule denies this operation.".into()
            } else if settings.policy == ApprovalPolicy::Never {
                "Approval is required for this request, but policy never does not permit approval prompts.".into()
            } else if settings.policy == ApprovalPolicy::Reject {
                "The configured reject policy refuses this approval request.".into()
            } else {
                "Permission mode denies this operation. Change permissions to proceed.".into()
            }
        } else if status == ApprovalStatus::Pending {
            if escalation {
                "This call requests access beyond the selected preset, within the server-authorized boundary. Approval applies only to this call.".into()
            } else if requested {
                let reason = arguments["__kyler_approval"]["reason"]
                    .as_str()
                    .unwrap_or("Review requested by the agent.");
                format!("Agent requested approval: {reason}")
            } else if settings.rules.get(name) == Some(&ToolRule::Ask) {
                "The configured tool rule requires approval.".into()
            } else if settings.policy == ApprovalPolicy::Untrusted {
                "Untrusted policy requires approval for this operation.".into()
            } else if matches!(name, "save_skill" | "install_skill") {
                "Saving reusable skill instructions requires confirmation.".into()
            } else if settings.reviewer == ApprovalReviewer::AutoReview {
                "Auto review needs confirmation for scripts, sensitive paths, destructive changes or operations outside its workspace rules.".into()
            } else {
                "This operation requires user approval.".into()
            }
        } else {
            "Allowed by the active permission rules.".into()
        };
        let identity = tool.approval_identity();
        let preview = if name == "apply_patch" {
            arguments["input"].as_str().unwrap_or_default().to_owned()
        } else {
            serde_json::to_string_pretty(arguments)?
        };
        // A human must be able to inspect the complete operation before approving it.
        ensure!(
            status != ApprovalStatus::Pending || preview.len() <= 32768,
            "Approval preview exceeds 32 KiB. Split the operation into smaller calls."
        );
        let mut record = ApprovalRecord {
            id: uuid::Uuid::new_v4().to_string(),
            run_id: context.run_id.clone(),
            session_id: context.session_id.clone(),
            origin_session_id: session.session_id.clone(),
            execution_id: session.current_execution()?.execution_id.clone(),
            tool_call_id: call.into(),
            tool: name.into(),
            operation: operation.label().into(),
            requires_user: status == ApprovalStatus::Pending,
            preview: crate::tools::output::text_prefix(&preview, 32768).to_owned(),
            working_directory: self.environment.as_ref().map(|e| {
                e.session_environment()
                    .working_directory
                    .to_string_lossy()
                    .into_owned()
            }),
            policy: policy.clone(),
            digest: format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&(&identity, arguments))?)
            ),
            status,
            reason,
            created_at: chrono::Utc::now().timestamp_millis(),
            expires_at: 0,
        };
        let index = session.current_execution()?.approvals.len();
        if status == ApprovalStatus::Pending {
            let ticket = context.manager.register(record, &identity, arguments)?;
            session
                .current_execution_mut()?
                .approvals
                .push(ticket.record.clone());
            self.checkpoint(session).await?;
            let mut waiting = Box::pin(ticket.wait());
            record = tokio::select! {
                result = &mut waiting => result,
                _ = changes.changed() => {
                    drop(waiting);
                    // The old ticket is invalidated before re-review.
                    session.current_execution_mut()?.approvals[index].status = ApprovalStatus::Cancelled;
                    session.current_execution_mut()?.approvals[index].reason = "Permission policy changed; operation will be reviewed again.".into();
                    self.checkpoint(session).await?;
                    return Box::pin(self.authorize_tool(session, name, call, arguments)).await;
                }
            };
            session.current_execution_mut()?.approvals[index] = record.clone();
        } else {
            session
                .current_execution_mut()?
                .approvals
                .push(record.clone());
        }
        self.checkpoint(session).await?;
        if record.status == ApprovalStatus::Denied && !record.requires_user {
            context.manager.publish(record.clone());
        }
        if changes.borrow().revision != policy.revision {
            return Box::pin(self.authorize_tool(session, name, call, arguments)).await;
        }
        ensure!(
            record.status == ApprovalStatus::Allowed,
            "Tool permission refused ({:?}): {}",
            record.status,
            record.reason
        );
        if policy.mode != PermissionMode::ReadOnly
            && !record.requires_user
            && crate::approval::policy::decide(
                &settings,
                operation,
                &clean_arguments,
                tool.as_ref(),
                effective_environment.as_deref(),
                requested,
            ) != ApprovalStatus::Allowed
        {
            return Box::pin(self.authorize_tool(session, name, call, arguments)).await;
        }
        preflight(effective_environment.as_deref())?;
        ensure!(
            identity == tool.approval_identity(),
            "Tool identity changed after approval; retry for a new review."
        );
        Ok(effective_environment)
    }
}
