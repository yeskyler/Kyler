//! Session permission choices apply to subsequent tool authorizations.
use crate::{
    error::{ApiError, ApiResult, valid_id},
    runs::{RunManager, persistence},
};

use std::{
    fs::File,
    io::{ErrorKind, Write},
};

use kyler_protocol::{
    api::{
        event::EventPayload,
        permissions::{
            ApprovalRecord, ApprovalResponse, PermissionMode, PermissionOption,
            PermissionSelection, PermissionSnapshot, PermissionState,
        },
    },
    approval::{ApprovalPolicy, ApprovalReviewer},
    sandbox::SandboxMode,
};

impl RunManager {
    pub(crate) async fn read_permissions(&self, id: &str) -> ApiResult<PermissionSnapshot> {
        match tokio::fs::read(self.root.join(format!("{id}.permissions"))).await {
            Ok(bytes) => serde_json::from_slice::<PermissionSnapshot>(&bytes)
                .map(|snapshot| {
                    let mut snapshot = normalize_permissions(snapshot);
                    let mode = self
                        .workspaces
                        .as_ref()
                        .map(|factory| factory.session_sandbox_mode())
                        .or_else(|| {
                            self.agent.execution_environment().map(|e| {
                                if e.policy().mode == SandboxMode::ReadOnly {
                                    SandboxMode::ReadOnly
                                } else {
                                    SandboxMode::WorkspaceWrite
                                }
                            })
                        });
                    if snapshot.mode == PermissionMode::FullAccess
                        && mode != Some(SandboxMode::DangerFullAccess)
                    {
                        snapshot.mode = if mode == Some(SandboxMode::ReadOnly) {
                            PermissionMode::ReadOnly
                        } else {
                            PermissionMode::WorkspaceAccess
                        };
                        if let Some(approval) = &mut snapshot.approval {
                            approval.policy = ApprovalPolicy::OnRequest;
                        }
                    }
                    snapshot
                })
                .map_err(|_| ApiError::bad("Invalid saved permissions.")),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(PermissionSnapshot {
                approval: Some({
                    let mut settings = self.approval_settings.clone();
                    settings.reviewer = ApprovalReviewer::User;
                    settings.policy = ApprovalPolicy::OnRequest;
                    settings
                }),
                mode: if self
                    .workspaces
                    .as_ref()
                    .map(|factory| factory.session_sandbox_mode())
                    .or_else(|| {
                        self.agent.execution_environment().map(|e| {
                            if e.policy().mode == SandboxMode::ReadOnly {
                                SandboxMode::ReadOnly
                            } else {
                                SandboxMode::WorkspaceWrite
                            }
                        })
                    })
                    == Some(SandboxMode::ReadOnly)
                {
                    PermissionMode::ReadOnly
                } else {
                    PermissionMode::WorkspaceAccess
                },
                revision: "initial".into(),
            }),
            Err(e) => Err(ApiError::storage(e.into())),
        }
    }
    pub async fn permission_state(
        &self,
        id: &str,
        selection: Option<PermissionSelection>,
    ) -> ApiResult<PermissionState> {
        valid_id(id)?;
        let registry = self.registry.lock().await;
        if self.approvals.is_none() {
            return Err(ApiError::bad(
                "Permission management is unavailable for this embedded runtime.",
            ));
        }
        let session = self.store.load(id).await.map_err(ApiError::storage)?;
        let agent = self.agent_for_session(&session)?;
        let environment = agent.execution_environment();
        let writes = environment
            .as_ref()
            .is_some_and(|e| !e.policy().filesystem.readwrite_paths.is_empty());
        let commands = environment
            .as_ref()
            .is_some_and(|e| e.policy().process.allow_commands);
        let network = environment.as_ref().is_some_and(|e| {
            !matches!(e.policy().network, kyler_core::sandbox::NetworkAccess::Deny)
        });
        let ceiling = format!(
            "Sandbox: {} · Commands: {} · Network: {}. Approval choices cannot expand this server boundary.",
            environment
                .as_ref()
                .map(|e| e.policy().mode.label())
                .unwrap_or("unavailable"),
            if commands { "allowed" } else { "blocked" },
            if network {
                "outbound / proxy"
            } else {
                "blocked"
            }
        );
        let options: Vec<_> = PermissionMode::ALL.into_iter().map(|mode| PermissionOption {
            mode, available: match mode {
                PermissionMode::ReadOnly => environment.is_some(),
                PermissionMode::WorkspaceAccess => writes,
                PermissionMode::FullAccess => environment.as_ref().is_some_and(|e| e.policy().mode == SandboxMode::DangerFullAccess),
                PermissionMode::AutoReview => false,
            }, description: match mode {
                PermissionMode::ReadOnly => "Read allowed files. Writes and network need an explicit approval request within the server boundary.",
                PermissionMode::WorkspaceAccess => if network { "Read allowed files, write within the workspace, and access the network. Tool approval and host rules still apply." } else { "Read allowed files and write within the workspace. Network access is disabled by the host configuration." },
                PermissionMode::FullAccess => "No local filesystem or network sandbox. No approval prompts. System permissions still apply.",
                PermissionMode::AutoReview => unreachable!(),
            }.into()
        }).collect();
        let changing = selection.is_some();
        let sandbox = environment.as_ref().map(|e| e.policy().mode);
        let selected = if let Some(selection) = selection {
            let previous = self.read_permissions(id).await?;
            let mode = selection.mode.unwrap_or(previous.mode).preset();
            if let Some(approval) = &selection.approval {
                if let Some(error) = approval.validation_error() {
                    return Err(ApiError::bad(error));
                }
            }
            if selection.mode.is_some() && !options.iter().any(|o| o.mode == mode && o.available) {
                return Err(ApiError::bad(
                    "This mode is unavailable under the server policy.",
                ));
            }
            if selection.mode.is_some()
                && mode == PermissionMode::FullAccess
                && !selection.confirm_full_access
            {
                return Err(ApiError::bad(
                    "Confirm the effective server ceiling before enabling Full access.",
                ));
            }
            let mut approval = selection.approval.unwrap_or_else(|| {
                let mut settings = self.approval_settings.clone();
                settings.policy = if mode == PermissionMode::FullAccess {
                    ApprovalPolicy::Never
                } else {
                    ApprovalPolicy::OnRequest
                };
                settings
            });
            approval.reviewer = ApprovalReviewer::User;
            if mode == PermissionMode::FullAccess {
                approval.policy = ApprovalPolicy::Never;
            }
            let snapshot = PermissionSnapshot {
                approval: Some(approval),
                mode,
                revision: uuid::Uuid::new_v4().to_string(),
            };
            let path = self.root.join(format!("{id}.permissions"));
            let root = self.root.clone();
            let bytes = serde_json::to_vec(&snapshot).map_err(|_| ApiError::internal())?;
            tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let temporary = root.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
                let result = (|| -> anyhow::Result<()> {
                    let mut file = persistence::private_file(&temporary)?;
                    file.write_all(&bytes)?;
                    file.sync_all()?;
                    std::fs::rename(&temporary, path)?;
                    File::open(root)?.sync_all()?;
                    Ok(())
                })();
                if result.is_err() {
                    let _ = std::fs::remove_file(temporary);
                }
                result
            })
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(ApiError::storage)?;
            snapshot
        } else {
            self.read_permissions(id).await?
        };
        if let Some(manager) = &self.approvals {
            if changing {
                manager.update_policy(id, selected.clone());
                if let Some(entry) = registry
                    .active
                    .get(id)
                    .and_then(|run| registry.runs.get(run))
                {
                    entry
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .info
                        .permissions = Some(selected.clone());
                    entry.publish(EventPayload::PermissionsBound {
                        policy: selected.clone(),
                    });
                }
            }
        }
        let active = registry
            .active
            .get(id)
            .and_then(|run| registry.runs.get(run))
            .and_then(|entry| entry.info().permissions);
        let sandbox = match selected.mode.preset() {
            PermissionMode::ReadOnly => Some(SandboxMode::ReadOnly),
            PermissionMode::WorkspaceAccess if writes => Some(SandboxMode::WorkspaceWrite),
            _ => sandbox,
        };
        Ok(PermissionState {
            sandbox,
            approval: selected.effective_approval(),
            selected,
            active,
            options,
            ceiling,
            workspace: session
                .environment
                .map(|e| e.workspace_root.to_string_lossy().into_owned()),
        })
    }
    pub async fn approval_list(&self, id: &str) -> ApiResult<Vec<ApprovalRecord>> {
        valid_id(id)?;
        let session = self.store.load(id).await.map_err(ApiError::storage)?;
        let mut records: Vec<_> = session
            .executions
            .iter()
            .chain(session.execution.iter())
            .flat_map(|e| e.approvals.clone())
            .collect();
        if let Some(manager) = &self.approvals {
            for record in manager.list(id) {
                records.retain(|r| r.id != record.id);
                records.push(record);
            }
        }
        records.sort_by_key(|r| (r.created_at, r.id.clone()));
        let keep = records.len().saturating_sub(256);
        records.drain(..keep);
        Ok(records)
    }
    pub fn approval_decide(
        &self,
        id: &str,
        response: &ApprovalResponse,
    ) -> ApiResult<ApprovalRecord> {
        valid_id(id)?;
        self.approvals
            .as_ref()
            .ok_or_else(|| ApiError::bad("Approval management is unavailable."))?
            .decide(id, response)
            .map_err(|_| {
                ApiError::new(
                    axum::http::StatusCode::CONFLICT,
                    "approval_stale",
                    "Approval is expired, cancelled, already resolved, or has a different digest.",
                )
            })
    }
}

fn normalize_permissions(mut snapshot: PermissionSnapshot) -> PermissionSnapshot {
    let mut settings = snapshot.effective_approval();
    settings.reviewer = ApprovalReviewer::User;
    snapshot.mode = snapshot.mode.preset();
    snapshot.approval = Some(settings);
    snapshot
}
