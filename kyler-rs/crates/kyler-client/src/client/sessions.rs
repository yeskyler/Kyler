//! Session creation, history, usage and deletion requests.
use super::{Client, valid_id};
use crate::error::{Error, Result};

use std::path::Path;

use reqwest::Method;

use kyler_protocol::api::{
    SessionUsage,
    history::{HistoryContent, HistoryEvent, HistoryPage},
    session::{CreateSessionRequest, MessagePage, SessionPage, SessionSummary},
};

impl Client {
    pub async fn session_usage(&self, id: &str) -> Result<SessionUsage> {
        valid_id(id)?;
        self.json(self.request(Method::GET, &format!("/v1/sessions/{id}/usage")))
            .await
    }
    pub async fn create_session_in(&self, workspace: &Path) -> Result<SessionSummary> {
        if !workspace.is_absolute() {
            return Err(Error::invalid("Workspace must be an absolute path."));
        }
        let path = workspace
            .to_str()
            .ok_or_else(|| Error::invalid("Workspace path must be UTF-8."))?;
        self.json(
            self.request(Method::POST, "/v1/sessions")
                .json(&CreateSessionRequest {
                    workspace_root: Some(path.into()),
                    draft: false,
                }),
        )
        .await
    }
    pub async fn create_session(&self) -> Result<SessionSummary> {
        self.json(self.request(Method::POST, "/v1/sessions")).await
    }
    pub async fn sessions(&self, cursor: usize, limit: usize) -> Result<SessionPage> {
        self.json(
            self.request(Method::GET, "/v1/sessions")
                .query(&[("cursor", cursor), ("limit", limit)]),
        )
        .await
    }
    pub async fn session(&self, id: &str) -> Result<SessionSummary> {
        valid_id(id)?;
        self.json(self.request(Method::GET, &format!("/v1/sessions/{id}")))
            .await
    }
    pub async fn history(&self, id: &str, cursor: usize, limit: usize) -> Result<HistoryPage> {
        valid_id(id)?;
        let result = self
            .json(
                self.request(Method::GET, &format!("/v1/sessions/{id}/history"))
                    .query(&[("cursor", cursor), ("limit", limit)]),
            )
            .await;
        match result {
            Err(error) if matches!(error.status, Some(404 | 405)) => {
                let page = self.messages(id, cursor, limit).await?;
                Ok(HistoryPage {
                    events: page
                        .messages
                        .into_iter()
                        .map(|message| HistoryEvent {
                            id: message.id,
                            execution_id: message.execution_id,
                            timestamp_ms: message.timestamp.saturating_mul(1000),
                            content: vec![HistoryContent::Message {
                                role: message.role,
                                text: message.content,
                            }],
                        })
                        .collect(),
                    executions: Vec::new(),
                    next_cursor: page.next_cursor,
                })
            }
            result => result,
        }
    }

    pub async fn messages(&self, id: &str, cursor: usize, limit: usize) -> Result<MessagePage> {
        valid_id(id)?;
        self.json(
            self.request(Method::GET, &format!("/v1/sessions/{id}/messages"))
                .query(&[("cursor", cursor), ("limit", limit)]),
        )
        .await
    }
    pub async fn delete_session(&self, id: &str) -> Result<()> {
        valid_id(id)?;
        let response = self
            .request(Method::DELETE, &format!("/v1/sessions/{id}"))
            .send()
            .await
            .map_err(|_| Error::transport())?;
        self.checked(response).await?;
        Ok(())
    }
}
