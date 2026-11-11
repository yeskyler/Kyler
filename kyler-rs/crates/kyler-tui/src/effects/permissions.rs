use super::{Effects, Event, Sender};
use crate::agent::AgentHandle;

use kyler_protocol::api::permissions::{
    ApprovalDecision, ApprovalRecord, ApprovalResponse, PermissionSelection,
};

impl Effects {
    pub fn watch_permissions(
        &mut self,
        client: AgentHandle,
        session: String,
        generation: u64,
        epoch: u64,
        send: Sender,
    ) {
        if let Some(previous) = self.permissions.take() {
            previous.abort();
        }
        self.permissions = Some(tokio::spawn(async move {
            if let Ok(state) = client.permissions(&session).await {
                if send
                    .send((generation, Event::Permissions(epoch, state)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            if let Ok(records) = client.approvals(&session).await {
                if send
                    .send((generation, Event::Approvals(records)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            if let Ok(records) = client.questions(&session).await {
                if send
                    .send((generation, Event::Questions(records)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }));
    }
    pub fn select_permissions(
        &mut self,
        client: AgentHandle,
        session: String,
        selection: PermissionSelection,
        generation: u64,
        epoch: u64,
        send: Sender,
    ) {
        self.spawn(async move {
            let event = match client.select_permissions(&session, &selection).await {
                Ok(state) => Event::PermissionsChanged(epoch, state),
                Err(e) => Event::PermissionsError(epoch, e.to_string()),
            };
            let _ = send.send((generation, event)).await;
        });
    }
    pub fn decide_approval(
        &mut self,
        client: AgentHandle,
        approval: ApprovalRecord,
        decision: ApprovalDecision,
        generation: u64,
        send: Sender,
    ) {
        self.spawn(async move {
            let event = match client
                .decide_approval(
                    &approval.id,
                    &ApprovalResponse {
                        decision,
                        digest: approval.digest,
                    },
                )
                .await
            {
                Ok(record) => Event::Approval(record),
                Err(e) => Event::ApprovalError(e.to_string()),
            };
            let _ = send.send((generation, event)).await;
        });
    }
}

impl Effects {
    pub fn answer_question(
        &mut self,
        client: AgentHandle,
        session: String,
        id: String,
        response: kyler_protocol::api::questions::QuestionResponse,
        generation: u64,
        send: Sender,
    ) {
        self.spawn(async move {
            let event = match client.answer_question(&session, &id, &response).await {
                Ok(record) => Event::Question(record),
                Err(error) => Event::QuestionError(id, error.to_string()),
            };
            let _ = send.send((generation, event)).await;
        });
    }
}
