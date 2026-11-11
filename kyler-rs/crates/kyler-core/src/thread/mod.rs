//! In-process agent threads. No listener, HTTP client, or service token is involved.
mod handle;
mod hosted;
mod manager;
mod preferences;
pub mod projection;
mod protocol;
mod runner;

use crate::{
    Agent, Session, SessionStore, approval::ApprovalManager, attachments::AttachmentStore,
    processes::ProcessManager, questions::QuestionManager,
};

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use tokio::sync::{Mutex, RwLock, mpsc};

use kyler_protocol::api::{permissions::PermissionSnapshot, run::RunInfo};

pub use manager::ThreadManager;

/// The UI has exactly one event consumer. Cloning a handle does not clone its receiver.
#[derive(Clone)]
pub struct ThreadHandle {
    pub(crate) inner: Arc<ThreadState>,
}
pub struct NewThread {
    pub thread: ThreadHandle,
    pub session_configured: Session,
}
pub use hosted::HostedThread;
pub use protocol::{HostedTurn, ThreadEvent, ThreadNotification, ThreadOp};
pub(crate) struct ThreadState {
    pub agent: Agent,
    pub environment: RwLock<Option<Arc<crate::ExecutionEnvironment>>>,
    pub session: RwLock<Session>,
    pub store: Arc<SessionStore>,
    pub approvals: Arc<ApprovalManager>,
    pub questions: Arc<QuestionManager>,
    pub processes: Arc<ProcessManager>,
    pub attachments: Arc<AttachmentStore>,
    pub permissions: RwLock<PermissionSnapshot>,
    pub default_approval: kyler_protocol::approval::ApprovalSettings,
    pub profile: RwLock<Option<String>>,
    pub active: RwLock<Option<RunInfo>>,
    pub cancel: std::sync::Mutex<Option<(String, crate::AgentCancelHandle)>>,
    pub source: Option<PathBuf>,
    pub preferences: PathBuf,
    pub hosted: bool,
    pub enforce_permissions: bool,
    pub event_timing: crate::diagnostics::ChannelTiming,
    pub ops: mpsc::Sender<ThreadOp>,
    pub events: Mutex<Option<mpsc::Receiver<ThreadEvent>>>,
    pub task: Mutex<Option<tokio::task::JoinHandle<Result<()>>>>,
}
