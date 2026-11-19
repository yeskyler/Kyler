//! Map internal presentation events into validated ACP schema values.
use agent_client_protocol::{
    Error, Result,
    v1::schema::{self, SessionNotification},
};
use serde_json::{Value, json};

use kyler_protocol::api::{event::EventPayload, permissions::ApprovalRecord};

pub(crate) fn notification(session: &str, update: Value) -> Result<SessionNotification> {
    serde_json::from_value(json!({"sessionId":session,"update":update}))
        .map_err(|_| Error::internal_error())
}
pub(crate) fn update(session: &str, event: &EventPayload) -> Result<Option<SessionNotification>> {
    let value = match event {
        EventPayload::TextDelta { text, .. } => {
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}})
        }
        EventPayload::ToolQueued { call_id, name, .. } => {
            json!({"sessionUpdate":"tool_call","toolCallId":call_id,"title":name,"status":"pending","kind":"other"})
        }
        EventPayload::ToolStarted { call_id, name, .. } => {
            json!({"sessionUpdate":"tool_call_update","toolCallId":call_id,"title":name,"status":"in_progress"})
        }
        EventPayload::ToolFinished {
            call_id,
            status,
            error,
            ..
        } => {
            json!({"sessionUpdate":"tool_call_update","toolCallId":call_id,"status":if status=="success" || status=="completed" {"completed"}else{"failed"},"content":error.as_ref().map(|text|vec![json!({"type":"content","content":{"type":"text","text":text}})]).unwrap_or_default()})
        }
        _ => return Ok(None),
    };
    notification(session, value).map(Some)
}
pub(crate) fn permission(
    session: &str,
    record: &ApprovalRecord,
) -> Result<schema::RequestPermissionRequest> {
    serde_json::from_value(json!({"sessionId":session,"toolCall":{"toolCallId":record.tool_call_id,"title":record.tool,"status":"pending","kind":"other","content":[{"type":"content","content":{"type":"text","text":record.preview}}]},"options":[{"optionId":"allow_once","name":"Allow once","kind":"allow_once"},{"optionId":"deny","name":"Deny","kind":"reject_once"}]})).map_err(|_|Error::internal_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kyler_protocol::api::permissions::{ApprovalStatus, PermissionMode, PermissionSnapshot};

    #[test]
    fn tool_updates_validate_and_permission_choices_never_grant_persistent_access() {
        let record = ApprovalRecord {
            id: "approval".into(),
            run_id: "run".into(),
            session_id: "session".into(),
            origin_session_id: "session".into(),
            execution_id: "execution".into(),
            tool_call_id: "call".into(),
            tool: "shell".into(),
            operation: "execute".into(),
            preview: "Run build".into(),
            requires_user: true,
            working_directory: None,
            policy: PermissionSnapshot {
                approval: None,
                mode: PermissionMode::WorkspaceAccess,
                revision: "revision".into(),
            },
            digest: "digest".into(),
            status: ApprovalStatus::Pending,
            reason: "Review".into(),
            created_at: 0,
            expires_at: 300,
        };
        let request = permission("session", &record).unwrap();
        assert_eq!(request.options.len(), 2);
        assert_eq!(
            request.options[0].kind,
            schema::PermissionOptionKind::AllowOnce
        );
        assert_eq!(
            request.options[1].kind,
            schema::PermissionOptionKind::RejectOnce
        );
        assert!(
            update(
                "session",
                &EventPayload::ToolStarted {
                    call_id: "call".into(),
                    name: "shell".into(),
                    summary: None,
                    activity: Default::default(),
                    source: Default::default()
                }
            )
            .unwrap()
            .is_some()
        );
        assert!(
            update(
                "session",
                &EventPayload::ToolFinished {
                    patch: None,
                    call_id: "call".into(),
                    name: "shell".into(),
                    status: "success".into(),
                    output_bytes: 10,
                    error: None
                }
            )
            .unwrap()
            .is_some()
        );
    }
}
