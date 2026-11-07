use crate::{
    context::{ContextMemory, Event, ExecutionContext, TokenUsage},
    environment::SessionEnvironment,
    session::Session,
    subagent::{ParentSessionRef, SubagentTask},
};

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ROLLOUT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RolloutPosition {
    pub last_seq: u64,
    pub log_offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolloutRecord {
    pub schema_version: u32,
    pub session_id: String,
    pub execution_id: Option<String>,
    pub seq: u64,
    pub timestamp_ms: i64,
    #[serde(flatten)]
    pub payload: RolloutPayload,
}

/// A checkpoint is a transaction: only records through CheckpointCommitted are visible.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RolloutPayload {
    SessionCreated {
        session: Box<Session>,
    },
    EventRecorded {
        event: Event,
    },
    ExecutionStarted {
        execution: ExecutionContext,
    },
    ExecutionUpdated {
        execution: ExecutionContext,
    },
    ExecutionFinished {
        execution: ExecutionContext,
    },
    ExecutionArchived {
        execution: ExecutionContext,
    },
    ContextCompacted {
        memory: ContextMemory,
    },
    ContextUpdated {
        memory: ContextMemory,
    },
    StateUpdated {
        set: HashMap<String, Value>,
        removed: Vec<String>,
    },
    UsageUpdated {
        usage: TokenUsage,
        subagent_usage: TokenUsage,
    },
    EnvironmentUpdated {
        environment: Option<SessionEnvironment>,
        parent: Option<ParentSessionRef>,
    },
    SubagentUpdated {
        task: SubagentTask,
    },
    SubagentRemoved {
        task_id: String,
    },
    CheckpointCommitted {
        revision: u64,
        updated_at: i64,
    },
}
