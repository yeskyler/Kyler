//! Composition boundary between the ACP adapter and the trusted local runtime.
use crate::{CliError, Result};

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Context;
use async_trait::async_trait;
use tokio::sync::mpsc;

use kyler_acp::{Host, HostEvent, HostSession, PlanEntry, PlanStatus};
use kyler_core::{
    runtime::UserTurn,
    thread::{ThreadEvent, ThreadHandle, ThreadManager},
};
use kyler_protocol::api::{permissions::ApprovalResponse, questions::QuestionResponse};

struct LocalHost {
    manager: ThreadManager,
    forwarders: tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}
impl LocalHost {
    async fn thread(&self, id: &str) -> anyhow::Result<ThreadHandle> {
        self.manager
            .get(id)
            .await
            .context("ACP session is unavailable")
    }
}
#[async_trait]
impl Host for LocalHost {
    async fn new_session(&self, cwd: &Path) -> anyhow::Result<HostSession> {
        let cwd = std::fs::canonicalize(cwd)?;
        anyhow::ensure!(cwd.is_dir(), "ACP cwd must be a directory");
        let new = self.manager.start_thread(&cwd, None).await?;
        let id = new.session_configured.session_id;
        let mut input = new.thread.take_events().await?;
        let (output, events) = mpsc::channel(128);
        let thread = new.thread;
        self.forwarders.lock().await.push(tokio::spawn(async move {
            while let Some(event) = input.recv().await {
                if let ThreadEvent::Event(event) = &event {
                    if matches!(&event.payload,kyler_protocol::api::event::EventPayload::ToolFinished {name,status,..} if name=="update_plan" && status=="success") {
                        if let Ok(Some(plan))=thread.execution_plan().await {
                            let entries=plan.steps.into_iter().map(|step|PlanEntry {content:step.content,status:match step.status {kyler_core::context::PlanStepStatus::Pending=>PlanStatus::Pending,kyler_core::context::PlanStepStatus::InProgress=>PlanStatus::InProgress,kyler_core::context::PlanStepStatus::Completed=>PlanStatus::Completed}}).collect();
                            if output.send(HostEvent::Plan {run_id:event.op_id.clone(),entries}).await.is_err() {let _=thread.cancel().await;}
                        }
                    }
                }
                let event = match event {
                    ThreadEvent::Event(event) => HostEvent::Update {
                        run_id: event.op_id,
                        payload: event.payload,
                    },
                    ThreadEvent::TurnFinished(info) => HostEvent::Finished(info),
                    ThreadEvent::ShutdownComplete => break,
                };
                if output.send(event).await.is_err() {
                    let _ = thread.cancel().await;
                    // Drain until shutdown so bounded presentation queues cannot deadlock cleanup.
                    while let Some(event) = input.recv().await {
                        if matches!(event, ThreadEvent::ShutdownComplete) {
                            break;
                        }
                    }
                    break;
                }
            }
        }));
        Ok(HostSession { id, events })
    }
    async fn submit(&self, id: &str, text: String) -> anyhow::Result<String> {
        self.thread(id)
            .await?
            .submit_turn(UserTurn {
                text,
                ..Default::default()
            })
            .await
    }
    async fn cancel(&self, id: &str) -> anyhow::Result<()> {
        self.thread(id).await?.cancel().await
    }
    async fn approve(
        &self,
        id: &str,
        approval: String,
        response: ApprovalResponse,
    ) -> anyhow::Result<()> {
        self.thread(id)
            .await?
            .respond_approval(approval, response)
            .await?;
        Ok(())
    }
    async fn dismiss_question(&self, id: &str, question: String) -> anyhow::Result<()> {
        self.thread(id)
            .await?
            .respond_question(id.into(), question, QuestionResponse::Cancel)
            .await?;
        Ok(())
    }
    async fn shutdown(&self) -> anyhow::Result<()> {
        let result = self.manager.shutdown().await;
        for task in self.forwarders.lock().await.drain(..) {
            let _ = task.await;
        }
        result
    }
}

pub async fn run(source: Option<PathBuf>) -> Result<()> {
    let config = kyler_config::loader::load_local(source.as_deref())
        .map_err(|error| CliError::new(2, error.to_string()))?;
    let worker = std::env::current_exe().map_err(CliError::io)?;
    let manager = ThreadManager::new_acp(config, source, worker)
        .await
        .map_err(|error| CliError::new(3, error.to_string()))?;
    kyler_acp::serve_stdio(Arc::new(LocalHost {
        manager,
        forwarders: Default::default(),
    }))
    .await
    .map_err(|_| CliError::new(3, "ACP connection stopped"))
}
