//! Apply asynchronous service results to the terminal application state.
use crate::{
    attachments::Paste,
    components::composer,
    effects::{Effects, Event, Sender},
    input::cancel,
    state::{Entry, Phase, State},
};

use kyler_client::Client;
use kyler_protocol::api::{attachments::MAX_ATTACHMENTS, permissions::ApprovalStatus};

pub(crate) fn apply(
    generation: u64,
    event: Event,
    state: &mut State,
    effects: &mut Effects,
    client: &Client,
    send: &Sender,
) {
    if generation != state.generation {
        return;
    }
    state.dirty = true;
    match event {
        Event::Questions(records) => {
            state.questions.replace(records);
            crate::questions::history(state);
        }
        Event::Question(record) => {
            state.questions.update(record);
            crate::questions::history(state);
        }
        Event::QuestionError(id, error) => {
            if state
                .questions
                .pending()
                .is_some_and(|record| record.id == id)
            {
                state.questions.busy = false;
                state.questions.error = Some(error);
            }
        }

        Event::ToolDetails(details) => {
            state.tool_details = Some(details);
            state.tool_scroll = 0;
        }
        Event::Rejected(error) => {
            state.entries.push(Entry::Error {
                title: "Request rejected".into(),
                message: error.message,
            });
            state.transcript_version += 1;
            state.failed_submission = state.pending.take();
            if !state.restore_failed_submission() {
                state.notice("Clear the draft and press Ctrl+R to restore failed input.");
            }
            state.phase = Phase::Ready;
        }
        Event::ImagePaste(epoch, result) => {
            if epoch != state.draft_epoch {
                return;
            }
            state.image_busy = false;
            match result {
                Ok(Paste::Image(image)) => {
                    if state.attachments.len() >= MAX_ATTACHMENTS {
                        state.notice("At most eight attachments are allowed per message.");
                    } else {
                        state.attachments.push(image);
                    }
                }
                Ok(Paste::Text(text)) => {
                    state.composer.insert_str(composer::clean(&text));
                }
                Err(error) => state.notice(error),
            }
        }
        Event::Permissions(epoch, policy) if epoch == state.permissions_epoch => {
            state.permissions = Some(policy)
        }
        Event::PermissionsChanged(epoch, policy) if epoch == state.permissions_epoch => {
            if state.permissions_open {
                state.panel_parent = None;
            }
            state.permissions_busy = false;
            state.permissions_error = None;
            state.permissions_open = false;
            state.permissions_confirm = false;
            state.active_permissions = policy.active.clone();
            state.permissions = Some(policy);
            if let Some(session) = state.session.clone() {
                effects.watch_permissions(client.clone(), session, generation, epoch, send.clone());
            }
        }
        Event::PermissionsError(epoch, error) if epoch == state.permissions_epoch => {
            state.permissions_busy = false;
            state.permissions_error = Some(error);
            if let Some(session) = state.session.clone() {
                effects.watch_permissions(client.clone(), session, generation, epoch, send.clone());
            }
        }
        Event::Permissions(..) | Event::PermissionsChanged(..) | Event::PermissionsError(..) => {}
        Event::Approvals(records) => {
            if state.phase == Phase::Detached && state.run.is_none() {
                if let Some(pending) = records.iter().find(|r| r.status == ApprovalStatus::Pending)
                {
                    state.run = Some(pending.run_id.clone());
                    state.phase = Phase::Running;
                    if let Some(session) = state.session.clone() {
                        effects.watch(
                            client.clone(),
                            session,
                            pending.run_id.clone(),
                            state.cursor,
                            generation,
                            send.clone(),
                        );
                    }
                }
            }
            for record in records {
                state.update_approval(record);
            }
        }
        Event::Approval(record) => {
            state.approval_busy = false;
            state.approval_error = None;
            state.update_approval(record);
        }
        Event::ApprovalError(error) => {
            state.approval_busy = false;
            state.approval_error = Some(error);
            state.dirty = true;
        }
        Event::Models(catalog, model) => {
            if state.model_applying {
                if state.model_open {
                    state.model_open = false;
                    state.model_details = false;
                    state.panel_parent = None;
                }
                state.model_applying = false;
            }
            state.model_busy = false;
            state.model_error = None;
            let previous = state
                .model_catalog
                .as_ref()
                .and_then(|c| c.models.get(state.model_selected))
                .map(|m| m.profile.as_str())
                .or_else(|| model.selected.as_ref().map(|m| m.profile.as_str()));
            state.model_selected = previous
                .and_then(|profile| catalog.models.iter().position(|m| m.profile == profile))
                .unwrap_or_else(|| {
                    state
                        .model_selected
                        .min(catalog.models.len().saturating_sub(1))
                });
            state.model_catalog = Some(catalog);
            state.session_model = Some(model);
            state.transcript_version += 1;
        }
        Event::ModelsError(error) => {
            state.model_applying = false;
            state.model_busy = false;
            state.model_error = Some(error);
        }
        Event::Loaded {
            session,
            info,
            usage,
            events,
            executions,
            truncated,
            active,
        } => {
            effects.watch_mcp(client.clone(), session.clone(), generation, send.clone());
            effects.watch_permissions(
                client.clone(),
                session.clone(),
                generation,
                state.permissions_epoch,
                send.clone(),
            );
            effects.skills(
                client.clone(),
                session.clone(),
                generation,
                send.clone(),
                true,
            );
            state.session = Some(session);
            crate::components::model_picker::refresh(state, effects, client, send, false, None);
            state.info = Some(info);
            state.usage = Some(usage);
            state.history(events, executions);
            if truncated {
                state.notice(
                    "Loaded the latest 2,000 events; use the history API for earlier events.",
                );
            }
            if active {
                state.phase = Phase::Detached;
                state.notice("This session has an active task. Use the CLI to inspect / cancel it, or create a new session.");
            }
        }
        Event::Skills(skills) => {
            state.skills = skills;
            state.skills_selected = state.skills_selected.min(state.skills.len() + 1);
            state.skills_busy = false;
            state.skills_error = None;
            if let Some(details) = &mut state.skill_details {
                if let Some(info) = state
                    .skills
                    .iter()
                    .find(|s| s.id == details.info.id && s.fingerprint == details.info.fingerprint)
                {
                    details.info = info.clone();
                } else {
                    state.skill_details = None;
                }
            }
        }
        Event::SkillDetails(details) => {
            state.skills_busy = false;
            if state
                .skills_selected
                .checked_sub(2)
                .and_then(|index| state.skills.get(index))
                .is_some_and(|info| {
                    info.id == details.info.id && info.fingerprint == details.info.fingerprint
                })
            {
                state.skill_details = Some(details);
                state.skills_scroll = 0;
                state.skills_error = None;
            }
        }
        Event::SkillsError(error) => {
            state.skills_busy = false;
            state.skills_error = Some(error);
        }
        Event::Mcp(servers) => {
            state.mcp = servers;
            state.mcp_error = None;
            state.mcp_selected = state.mcp_selected.min(state.mcp.len().saturating_sub(1));
        }
        Event::McpError(error) => state.mcp_error = Some(error),
        Event::Accepted(run) => {
            state.run = Some(run.clone());
            crate::components::model_picker::refresh(state, effects, client, send, false, None);
            if state.cancel_requested {
                cancel(state, effects, client, send);
            } else {
                state.phase = Phase::Running;
            }
        }
        Event::Stream(event) => {
            let finished = matches!(event, kyler_client::ClientEvent::Result(_));
            state.stream(event);
            if finished {
                if let Some(session) = state.session.clone() {
                    effects.skills(client.clone(), session, generation, send.clone(), true);
                }
                crate::components::model_picker::refresh(state, effects, client, send, false, None);
            }
        }
        Event::Usage(usage) => {
            if state
                .usage
                .as_ref()
                .is_none_or(|old| usage.total_tokens >= old.total_tokens)
            {
                state.usage = Some(usage);
            }
        }
        Event::Error(error) => {
            state.phase = Phase::Detached;
            state.notice(error);
        }
        Event::CancelError(error) => state.notice(error),
        Event::Cancelled => {
            if state.cancel_requested {
                state.notice("Cancellation requested; waiting for server confirmation…");
            }
        }
    }
}
