//! Release run capacity while awaiting user input, then reacquire before the agent continues.
use anyhow::Result;
use kyler_core::questions::QuestionWaiter;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

pub(super) struct QuestionPermit {
    permit: Mutex<Option<OwnedSemaphorePermit>>,
    semaphore: Arc<Semaphore>,
}
impl QuestionPermit {
    pub fn new(permit: OwnedSemaphorePermit, semaphore: Arc<Semaphore>) -> Self {
        Self {
            permit: Mutex::new(Some(permit)),
            semaphore,
        }
    }
}
#[async_trait::async_trait]
impl QuestionWaiter for QuestionPermit {
    async fn suspend(&self) -> Result<()> {
        self.permit.lock().await.take();
        Ok(())
    }
    async fn resume(&self) -> Result<()> {
        let mut permit = self.permit.lock().await;
        if permit.is_none() {
            *permit = Some(self.semaphore.clone().acquire_owned().await?);
        }
        Ok(())
    }
}
