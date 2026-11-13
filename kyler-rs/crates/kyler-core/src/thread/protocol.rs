//! Operations are trusted in-process commands; HTTP payloads never construct host contexts.
use crate::{Agent, Session, runtime::UserTurn};

use std::time::Duration;

use anyhow::Result;
use tokio::sync::oneshot;

use kyler_protocol::api::{
    models::SessionModel,
    permissions::{ApprovalRecord, ApprovalResponse, PermissionSelection, PermissionState},
    questions::{QuestionRecord, QuestionResponse},
    run::RunInfo,
};

/// A host-authorized, already admitted turn. Never deserialized from a client request.
pub struct HostedTurn {
    pub agent: Agent,
    pub session: Session,
    pub info: RunInfo,
    pub timeout: Duration,
}
pub enum ThreadOp {
    SubmitTurn {
        op_id: String,
        turn: UserTurn,
        context: Option<Box<HostedTurn>>,
        reply: oneshot::Sender<Result<String>>,
    },
    CancelTurn {
        reply: oneshot::Sender<()>,
    },
    RespondApproval {
        id: String,
        response: ApprovalResponse,
        reply: oneshot::Sender<Result<ApprovalRecord>>,
    },
    RespondQuestion {
        session_id: String,
        id: String,
        response: QuestionResponse,
        reply: oneshot::Sender<Result<QuestionRecord>>,
    },
    SelectModel {
        profile: Option<String>,
        reply: oneshot::Sender<Result<SessionModel>>,
    },
    SelectThinking {
        level: kyler_protocol::api::models::ThinkingLevel,
        reply: oneshot::Sender<Result<kyler_protocol::api::models::ThinkingState>>,
    },
    SelectPermissions {
        selection: PermissionSelection,
        reply: oneshot::Sender<Result<PermissionState>>,
    },
    Shutdown,
}

/// Sequence numbers and operation identity are independent of HTTP/SSE subscription cursors.
#[derive(Debug, Clone)]
pub struct ThreadNotification {
    pub thread_id: String,
    pub op_id: String,
    pub execution_id: Option<String>,
    pub seq: u64,
    pub timestamp_ms: i64,
    pub payload: kyler_protocol::api::event::EventPayload,
}
#[derive(Debug, Clone)]
pub enum ThreadEvent {
    Event(ThreadNotification),
    TurnFinished(RunInfo),
    ShutdownComplete,
}
