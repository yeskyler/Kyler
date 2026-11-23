use super::{
    events::Entry,
    manager::{RunManager, accepted},
    persistence,
};
use crate::error::{ApiError, ApiResult, valid_id};

use std::sync::Arc;

use axum::http::StatusCode;
use sha2::{Digest, Sha256};

use kyler_core::runtime::UserTurn;
use kyler_protocol::api::{
    attachments::{AttachmentInfo, AttachmentKind, MAX_ATTACHMENTS},
    images::MAX_IMAGES,
    run::{CreateRunRequest, RunAccepted, RunInfo, RunStatus, SessionCommand},
};

impl RunManager {
    /// Admission survives an HTTP client disconnect while the manifest is written.
    pub async fn start(
        self: &Arc<Self>,
        session_id: String,
        request: CreateRunRequest,
    ) -> ApiResult<RunAccepted> {
        self.start_as(
            kyler_protocol::api::gateway::Principal::local(),
            session_id,
            request,
        )
        .await
    }
    pub async fn start_as(
        self: &Arc<Self>,
        principal: kyler_protocol::api::gateway::Principal,
        session_id: String,
        mut request: CreateRunRequest,
    ) -> ApiResult<RunAccepted> {
        if !principal.role.executes()
            || !self
                .security
                .active(&principal)
                .await
                .map_err(ApiError::storage)?
        {
            return Err(crate::gateway::forbidden());
        }
        if let Some(key) = &request.client_request_id {
            if key.is_empty() || key.len() > 128 {
                return Err(ApiError::bad(
                    "client_request_id must contain 1 to 128 bytes.",
                ));
            }
            request.client_request_id = Some(format!(
                "{:x}",
                Sha256::digest(format!("{}\0{key}", principal.id))
            ));
        }
        let manager = self.clone();
        tokio::spawn(async move { manager.admit(session_id, request, principal).await })
            .await
            .map_err(|_| ApiError::internal())?
    }
    pub(super) async fn admit(
        self: Arc<Self>,
        session_id: String,
        request: CreateRunRequest,
        principal: kyler_protocol::api::gateway::Principal,
    ) -> ApiResult<RunAccepted> {
        valid_id(&session_id)?;
        if request.command.is_some()
            && (!request.input.is_empty()
                || !request.skills.is_empty()
                || !request.images.is_empty()
                || !request.attachment_ids.is_empty()
                || request.resume)
        {
            return Err(ApiError::bad(
                "Session commands must be sent without message input, attachments, or resume.",
            ));
        }
        if request.input.trim().is_empty()
            && request.skills.is_empty()
            && request.images.is_empty()
            && request.attachment_ids.is_empty()
            && !request.resume
            && request.command.is_none()
        {
            return Err(ApiError::bad("Input cannot be empty."));
        }
        kyler_core::skills::validate_references(&request.skills)
            .map_err(|error| ApiError::bad(&error.to_string()))?;
        if request.input.len() > self.settings.max_request_bytes {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request_too_large",
                "Input exceeds the request limit.",
            ));
        }
        if request
            .client_request_id
            .as_ref()
            .is_some_and(|key| key.is_empty() || key.len() > 128)
        {
            return Err(ApiError::bad(
                "client_request_id must contain 1 to 128 bytes.",
            ));
        }
        if request.images.len() > MAX_IMAGES {
            return Err(ApiError::bad(
                "At most four images are allowed per message.",
            ));
        }
        let bytes = if let Some(command) = &request.command {
            serde_json::to_vec(command)
        } else if request.skills.is_empty() {
            serde_json::to_vec(&(
                &request.input,
                &request.images,
                &request.attachment_ids,
                request.resume,
            ))
        } else {
            serde_json::to_vec(&(
                &request.input,
                &request.images,
                &request.attachment_ids,
                request.resume,
                &request.skills,
            ))
        }
        .map_err(|_| ApiError::internal())?;
        let fingerprint = format!("{:x}", Sha256::digest(bytes));
        let raw_images = request.images;
        let images = tokio::task::spawn_blocking(move || {
            kyler_core::attachments::image::prepare_images(&raw_images)
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|error| ApiError::bad(&format!("{error:#}")))?;
        let mut registry = self.registry.lock().await;
        if *self.stopping.borrow() {
            return Err(ApiError::stopping());
        }
        if let Some(key) = &request.client_request_id {
            if let Some((id, input)) = registry.keys.get(&(session_id.clone(), key.clone())) {
                if input != &fingerprint {
                    return Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "idempotency_conflict",
                        "The request ID was used with different input.",
                    ));
                }
                let entry=registry.runs.get(id).ok_or_else(||ApiError::new(StatusCode::CONFLICT,"admission_outcome_unknown","The previous admission was interrupted. Inspect session state; automatic resubmission is blocked."))?;
                return Ok(accepted(&entry.info()));
            }
        }
        if registry.active.contains_key(&session_id) {
            return Err(ApiError::conflict());
        }
        let permit = self.permits.clone().try_acquire_owned().map_err(|_| {
            ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "capacity_exceeded",
                "The concurrent run limit has been reached.",
            )
        })?;
        let session = self
            .store
            .load(&session_id)
            .await
            .map_err(ApiError::storage)?;
        if session.is_running() {
            return Err(ApiError::conflict());
        }
        if request.resume {
            if !request.input.trim().is_empty()
                || !images.is_empty()
                || !request.attachment_ids.is_empty()
            {
                return Err(ApiError::bad(
                    "Resume cannot include replacement input or attachments.",
                ));
            }
            if session.execution.as_ref().is_none_or(|execution| {
                !matches!(
                    execution.status,
                    kyler_core::ExecutionStatus::Failed | kyler_core::ExecutionStatus::Cancelled
                )
            }) {
                return Err(ApiError::bad(
                    "Only failed or cancelled tasks can be resumed.",
                ));
            }
        }

        let workspace = session.environment.as_ref().map(|environment| {
            environment
                .project_root()
                .cloned()
                .unwrap_or_else(|| environment.cwd.clone())
        });
        if !self
            .security
            .session_allowed(&principal, &session_id, workspace.as_deref())
            .await
            .map_err(ApiError::storage)?
        {
            return Err(crate::gateway::forbidden());
        }
        self.validate_session_environment(&session)?;
        match &request.command {
            Some(SessionCommand::Review { .. } | SessionCommand::Memory)
                if self.agent.subagent_manager().is_none() =>
            {
                return Err(ApiError::bad("Subagents are disabled on this server."));
            }
            Some(SessionCommand::Compact | SessionCommand::Memory) if session.events.is_empty() => {
                return Err(ApiError::bad(
                    "This session has no conversation history yet.",
                ));
            }
            _ => {}
        }
        let compact = matches!(request.command, Some(SessionCommand::Compact));
        let mut input = request
            .command
            .as_ref()
            .map(SessionCommand::input)
            .unwrap_or(request.input);

        let selection = self.read_model_selection(&session_id).await?;
        let mut agent = self
            .agent_for_session(&session)?
            .bind_model(selection.profile.as_deref())
            .map_err(|e| ApiError::bad(&e.to_string()))?;
        if !principal.role.administrative() {
            if let Some(environment) = agent.execution_environment() {
                let environment = environment
                    .with_workspace_boundary()
                    .map_err(|_| crate::gateway::forbidden())?;
                agent = agent.with_environment(Arc::new(environment));
            }
        }
        if !principal.scope.models.is_empty()
            && !agent.model_info().is_some_and(|model| {
                kyler_gateway::security::Security::model_allowed(&principal, &model.profile)
            })
        {
            return Err(crate::gateway::forbidden());
        }
        if let Some(SessionCommand::Review { target }) = &request.command
            && let Err(error) = agent.validate_review_target(target.clone()).await
        {
            let status = if agent.effective_execution_environment().is_none() {
                "This session has no workspace available for code review."
            } else if error.to_string().contains("not a git repository") {
                "The current session workspace is not a Git repository. No Git change set is available to review there."
            } else {
                "The requested Git target could not be verified in the current session workspace."
            };
            input = format!(
                "I requested a code review. The workspace check reported: {status} Reply directly in the conversation in the user's language. If the current workspace path is available in your context, include it. Explain what was unavailable without claiming that a review was performed. Ask me to open a repository with changes or provide a repository path, commit ID, or diff so the review can continue. Do not start a review task until reviewable input is available."
            );
        }
        if !images.is_empty() {
            agent.validate_image_input().map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "model_image_unsupported", "The selected model does not support image input. Select a vision-capable model with /model, then resend."))?;
        }
        if request.attachment_ids.len() > MAX_ATTACHMENTS {
            return Err(ApiError::bad("Too many attachments."));
        }
        let mut attached = Vec::new();
        for id in &request.attachment_ids {
            let info = self
                .attachments
                .get(&session_id, id)
                .map_err(|_| ApiError::bad("Attachment is unavailable in this session."))?;
            if !attached
                .iter()
                .any(|existing: &AttachmentInfo| existing.id == info.id)
            {
                attached.push(info);
            }
        }
        if attached
            .iter()
            .any(|attachment| attachment.kind == AttachmentKind::Image)
        {
            agent.validate_image_input().map_err(|_|ApiError::new(StatusCode::BAD_REQUEST,"model_image_unsupported","The selected model does not support image input. Select a vision-capable model with /model."))?;
        }
        if attached
            .iter()
            .any(|attachment| attachment.kind != AttachmentKind::Image)
        {
            agent.validate_document_input().map_err(|_|ApiError::new(StatusCode::BAD_REQUEST,"model_tools_unsupported","The selected model cannot read attachments using tools. Select a tool-capable model with /model."))?;
        }
        let run_id = uuid::Uuid::new_v4().to_string();
        // A scoped caller must never reuse a connector process launched under an
        // administrator's broader environment, even when sharing the same session.
        let scoped_mcp = if !principal.role.administrative() {
            let scope = format!("gateway:{}:{run_id}", principal.id);
            agent = agent.with_mcp_scope(scope.clone());
            agent.mcp_manager().cloned().map(|manager| (manager, scope))
        } else {
            None
        };

        let permission = if self.approvals.is_some() {
            let policy = self
                .core_thread(&session)
                .await?
                .handle
                .permissions(None)
                .await
                .map_err(|error| ApiError::bad(&error.to_string()))?
                .selected;
            Some(policy)
        } else {
            None
        };
        if !principal.role.administrative()
            && permission.as_ref().is_some_and(|permission| {
                permission.mode == kyler_protocol::api::permissions::PermissionMode::FullAccess
            })
        {
            return Err(crate::gateway::forbidden());
        }
        self.prune_locked(&mut registry, true).await?;
        if *self.stopping.borrow() {
            return Err(ApiError::stopping());
        }
        let info = RunInfo {
            run_id,
            permissions: permission,
            session_id: session_id.clone(),
            model: agent.model_info(),
            execution_id: None,
            status: RunStatus::Starting,
            created_at: chrono::Utc::now().timestamp_millis(),
            finished_at: None,
            last_event_seq: 0,
            output: None,
            usage: None,
            error: None,
        };
        self.security
            .store
            .bind(
                "run".into(),
                info.run_id.clone(),
                principal.id.clone(),
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .map_err(ApiError::storage)?;
        if let Some(key) = &request.client_request_id {
            persistence::save_admission(
                self.root.clone(),
                persistence::Admission {
                    run_id: info.run_id.clone(),
                    session_id: session_id.clone(),
                    key: key.clone(),
                    fingerprint: fingerprint.clone(),
                },
            )
            .await
            .map_err(ApiError::storage)?;
            registry.keys.insert(
                (session_id.clone(), key.clone()),
                (info.run_id.clone(), fingerprint.clone()),
            );
        }
        persistence::save(self.root.clone(), info.clone())
            .await
            .map_err(ApiError::storage)?;
        let entry = Arc::new(Entry::new(
            info.clone(),
            self.settings.event_buffer_max_events,
            self.settings.event_buffer_max_bytes,
            false,
            self.settings.max_subscribers_per_run,
        ));
        *entry.input.lock().unwrap_or_else(|e| e.into_inner()) = Some(input.clone());
        registry
            .active
            .insert(session_id.clone(), info.run_id.clone());
        registry.runs.insert(info.run_id.clone(), entry.clone());
        if let Some(key) = request.client_request_id {
            registry
                .keys
                .insert((session_id, key), (info.run_id.clone(), fingerprint));
        }
        let manager = self.clone();
        tokio::spawn(async move {
            let result = manager
                .drive(
                    entry.clone(),
                    session,
                    UserTurn {
                        skills: request.skills,
                        text: input,
                        images,
                        attachments: attached,
                        resume: request.resume,
                        compact,
                    },
                    agent,
                )
                .await;
            if let Err(error) = result {
                tracing::error!(run_id=%entry.info().run_id,"run driver failed");
                manager.finish(&entry, None, Some(error)).await;
            }
            let mut registry = manager.registry.lock().await;
            let finished = entry.info();
            if registry.active.get(&finished.session_id) == Some(&finished.run_id) {
                registry.active.remove(&finished.session_id);
                // HTTP deduplication and session history are durable; idle actors can be
                // released without stopping the host's shared tools or connections.
                if manager.threads.close(&finished.session_id).await.is_err() {
                    tracing::warn!("Unable to join an idle core thread");
                }
            }
            drop(registry);
            drop(permit);
            if let Some((mcp, scope)) = scoped_mcp {
                mcp.close_session(&scope).await;
            }
        });
        Ok(accepted(&info))
    }
}
