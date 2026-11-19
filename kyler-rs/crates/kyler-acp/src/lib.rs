//! ACP v1 transport and presentation; execution remains behind a host boundary.
mod agent;
mod updates;

use std::{path::Path, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

use kyler_protocol::api::{event::EventPayload, permissions::ApprovalResponse, run::RunInfo};

pub use agent::{serve, serve_stdio};

pub struct HostSession {
    pub id: String,
    pub events: mpsc::Receiver<HostEvent>,
}
pub struct PlanEntry {
    pub content: String,
    pub status: PlanStatus,
}
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

pub enum HostEvent {
    Plan {
        run_id: String,
        entries: Vec<PlanEntry>,
    },
    Update {
        run_id: String,
        payload: EventPayload,
    },
    Finished(RunInfo),
}

/// A host validates workspace authority, owns persistence, and runs the agent.
#[async_trait]
pub trait Host: Send + Sync + 'static {
    async fn new_session(&self, cwd: &Path) -> Result<HostSession>;
    async fn submit(&self, session: &str, text: String) -> Result<String>;
    async fn cancel(&self, session: &str) -> Result<()>;
    async fn approve(&self, session: &str, id: String, response: ApprovalResponse) -> Result<()>;
    async fn dismiss_question(&self, session: &str, id: String) -> Result<()>;
    async fn shutdown(&self) -> Result<()>;
}

pub(crate) type SharedHost = Arc<dyn Host>;
