//! Session permission choices apply to subsequent tool authorizations.
use crate::{
    error::{ApiError, ApiResult, valid_id},
    runs::RunManager,
};

use std::io::ErrorKind;

use kyler_protocol::{
    api::permissions::{
        ApprovalRecord, ApprovalResponse, PermissionMode, PermissionSelection, PermissionSnapshot,
        PermissionState,
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
        let thread = self.core_thread(&session).await?;
        let state = thread
            .handle
            .permissions(selection.as_ref())
            .await
            .map_err(|error| ApiError::bad(&error.to_string()))?;
        if selection.is_some() {
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
                    .permissions = Some(state.selected.clone());
            }
        }
        Ok(state)
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
    pub async fn approval_decide(
        &self,
        id: &str,
        response: &ApprovalResponse,
    ) -> ApiResult<ApprovalRecord> {
        valid_id(id)?;
        let manager = self
            .approvals
            .as_ref()
            .ok_or_else(|| ApiError::bad("Approval management is unavailable."))?;
        let record = manager.get(id).ok_or_else(|| {
            ApiError::new(
                axum::http::StatusCode::CONFLICT,
                "approval_stale",
                "Approval is expired, cancelled, already resolved, or has a different digest.",
            )
        })?;
        let session = self
            .store
            .load(&record.session_id)
            .await
            .map_err(ApiError::storage)?;
        self.core_thread(&session)
            .await?
            .handle
            .respond_approval(id.into(), response.clone())
            .await
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
