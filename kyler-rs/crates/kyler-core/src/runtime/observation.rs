//! Optional local timing observations, independent of durable conversation events.
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum RuntimeObservation {
    ModelRequestStarted,
    FirstModelEvent,
    McpSnapshot {
        elapsed: Duration,
    },
    SkillSnapshot {
        elapsed: Duration,
    },
    ToolSnapshot {
        fingerprint: String,
        names: Vec<String>,
    },
}

/// Callbacks run inline and must be nonblocking. No prompts, outputs or credentials are sent.
pub trait RuntimeObserver: Send + Sync {
    fn observe(&self, observation: RuntimeObservation);
}

impl super::Agent {
    pub(super) fn observe(&self, observation: RuntimeObservation) {
        if let Some(observer) = &self.observer {
            observer.observe(observation);
        }
    }
}
