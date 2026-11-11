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
    run::{CreateRunRequest, RunAccepted, RunInfo, RunStatus},
};

impl RunManager {
    /// Admission survives an HTTP client disconnect while the manifest is written.
    pub async fn start(
        self: &Arc<Self>,
        session_id: String,
        request: CreateRunRequest,
    ) -> ApiResult<RunAccepted> {
        let manager = self.clone();
        tokio::spawn(async move { manager.admit(session_id, request).await })
            .await
            .map_err(|_| ApiError::internal())?
    }
    pub(super) async fn admit(
        self: Arc<Self>,
        session_id: String,
        request: CreateRunRequest,
    ) -> ApiResult<RunAccepted> {
        valid_id(&session_id)?;
        if request.input.trim().is_empty()
            && request.images.is_empty()
            && request.attachment_ids.is_empty()
            && !request.resume
        {
            return Err(ApiError::bad("Input cannot be empty."));
        }
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
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    &request.input,
                    &request.images,
                    &request.attachment_ids,
                    request.resume
                ))
                .map_err(|_| ApiError::internal())?
            )
        );
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
        let mut session = self
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

        if self.isolate_session(&mut session).await? {
            self.store
                .save(&mut session)
                .await
                .map_err(ApiError::storage)?;
        }
        let selection = self.read_model_selection(&session_id).await?;
        let agent = self
            .agent_for_session(&session)?
            .bind_model(selection.profile.as_deref())
            .map_err(|e| ApiError::bad(&e.to_string()))?;
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
        registry
            .active
            .insert(session_id.clone(), info.run_id.clone());
        registry.runs.insert(info.run_id.clone(), entry.clone());
        if let Some(key) = request.client_request_id {
            registry
                .keys
                .insert((session_id, key), (info.run_id.clone(), fingerprint));
        }
        let permit = Arc::new(super::questions::QuestionPermit::new(
            permit,
            self.permits.clone(),
        ));
        let agent = agent.with_questions(self.questions.clone(), Some(permit.clone()));
        let manager = self.clone();
        tokio::spawn(async move {
            let result = manager
                .drive(
                    entry.clone(),
                    session,
                    UserTurn {
                        text: request.input,
                        images,
                        attachments: attached,
                        resume: request.resume,
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
            drop(permit);
        });
        Ok(accepted(&info))
    }
}
