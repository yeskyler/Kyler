//! Live execution notifications, separate from durable session events.
use super::{Agent, AgentResult};
use crate::{
    attachments::image::UserImage,
    context::{ExecutionStatus, ToolResultStatus},
    session::Session,
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

use kyler_protocol::activity::{ActivityKind, ToolSource};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentStreamEvent {
    ContextCompaction {
        compaction: kyler_protocol::api::compaction::CompactionInfo,
    },
    Started {
        execution_id: String,
        session_id: String,
    },
    StepStarted {
        step: u32,
    },
    ReasoningDelta {
        step: u32,
        text: String,
    },
    TextDelta {
        step: u32,
        text: String,
    },
    ToolCallDelta {
        call_id: String,
        name: String,
    },
    ToolQueued {
        call_id: String,
        name: String,
        arguments: serde_json::Value,
        #[serde(default)]
        activity: ActivityKind,
        #[serde(default)]
        source: ToolSource,
    },
    ToolStarted {
        call_id: String,
        name: String,
        arguments: serde_json::Value,
        #[serde(default)]
        activity: ActivityKind,
        #[serde(default)]
        source: ToolSource,
    },
    ToolFinished {
        call_id: String,
        name: String,
        status: ToolResultStatus,
        output: String,
    },
    Completed {
        execution_id: String,
    },
    Failed {
        message: String,
    },
    Cancelled,
}

/// Always returns the owned session, including after cancellation or model/tool failure.
#[derive(Debug)]
pub struct AgentStreamOutcome {
    pub session: Session,
    pub result: Result<AgentResult>,
}

/// Bounded live events and an owned execution task. Drop requests cooperative cancellation.
/// Call finish/finish_with_session to await cleanup and final persistence.
pub struct AgentStream {
    receiver: mpsc::Receiver<AgentStreamEvent>,
    cancel: watch::Sender<bool>,
    task: Option<JoinHandle<AgentStreamOutcome>>,
}

#[derive(Clone)]
pub struct AgentCancelHandle {
    cancel: watch::Sender<bool>,
}
impl AgentCancelHandle {
    pub fn cancel(&self) {
        self.cancel.send_replace(true);
    }
}

pub(super) struct StreamControl {
    sender: mpsc::Sender<AgentStreamEvent>,
    cancel: watch::Receiver<bool>,
    timing: crate::diagnostics::ChannelTiming,
}

impl StreamControl {
    pub(super) fn is_cancelled(&self) -> bool {
        *self.cancel.borrow()
    }
    pub(super) async fn cancelled(&self) {
        let mut cancel = self.cancel.clone();
        let _ = cancel.wait_for(|value| *value).await;
    }
    pub(super) async fn emit(&self, event: AgentStreamEvent) -> Result<()> {
        tokio::select! {
            biased;
            _ = self.cancelled() => anyhow::bail!("agent execution cancelled"),
            result = self.timing.send(&self.sender, event) => result.map_err(|_| anyhow::anyhow!("stream consumer disconnected")),
        }
    }
}

impl AgentStream {
    pub(super) fn start(agent: Agent, session: Session, input: String) -> Self {
        Self::start_with_images(agent, session, input, Vec::new())
    }
    pub(super) fn start_with_images(
        agent: Agent,
        session: Session,
        input: String,
        images: Vec<UserImage>,
    ) -> Self {
        Self::start_turn(
            agent,
            session,
            super::UserTurn {
                text: input,
                images,
                ..Default::default()
            },
        )
    }
    pub(super) fn start_turn(agent: Agent, mut session: Session, turn: super::UserTurn) -> Self {
        let (sender, receiver) = mpsc::channel(64);
        let (cancel, cancelled) = watch::channel(false);
        let task = tokio::spawn(async move {
            let control = StreamControl {
                sender,
                cancel: cancelled,
                timing: crate::diagnostics::ChannelTiming::new(
                    "agent_to_runner",
                    &session.session_id,
                ),
            };
            let result = agent.execute_turn(&mut session, turn, Some(&control)).await;
            let result = result.and_then(|output| {
                Ok(AgentResult {
                    output,
                    context: session.current_execution()?.clone(),
                    session: session.clone(),
                })
            });
            let terminal = match &result {
                Ok(result) => AgentStreamEvent::Completed {
                    execution_id: result.context.execution_id.clone(),
                },
                Err(_)
                    if session
                        .execution
                        .as_ref()
                        .is_some_and(|e| e.status == ExecutionStatus::Cancelled) =>
                {
                    AgentStreamEvent::Cancelled
                }
                Err(error) => AgentStreamEvent::Failed {
                    message: format!("{error:#}"),
                },
            };
            // Terminal notification follows cleanup/checkpoint, even after explicit cancellation.
            let _ = control.sender.send(terminal).await;
            AgentStreamOutcome { session, result }
        });
        Self {
            receiver,
            cancel,
            task: Some(task),
        }
    }

    pub async fn next(&mut self) -> Option<AgentStreamEvent> {
        self.receiver.recv().await
    }

    pub fn cancel_handle(&self) -> AgentCancelHandle {
        AgentCancelHandle {
            cancel: self.cancel.clone(),
        }
    }

    pub fn cancel(&self) {
        self.cancel.send_replace(true);
    }

    /// Drains remaining events to avoid backpressure deadlocks when no consumer is polling.
    pub async fn finish(self) -> Result<AgentResult> {
        self.finish_with_session().await?.result
    }

    pub async fn finish_with_session(mut self) -> Result<AgentStreamOutcome> {
        while self.receiver.recv().await.is_some() {}
        Ok(self.task.take().expect("execution task exists").await?)
    }
}

impl Drop for AgentStream {
    fn drop(&mut self) {
        self.cancel.send_replace(true);
    }
}

#[cfg(test)]
pub(super) fn compaction_test_stream() -> (
    StreamControl,
    mpsc::Receiver<AgentStreamEvent>,
    watch::Sender<bool>,
) {
    let (sender, receiver) = mpsc::channel(32);
    let (cancel, cancelled) = watch::channel(false);
    (
        StreamControl {
            sender,
            cancel: cancelled,
            timing: crate::diagnostics::ChannelTiming::new("agent_to_runner", "test"),
        },
        receiver,
        cancel,
    )
}
