//! Read-only process and tool-call inspection requests.
use super::{Client, valid_id};
use crate::error::{Error, Result};

use reqwest::Method;

use kyler_protocol::api::{
    processes::ProcessInfo,
    tasks::{TaskAction, TaskInfo, TaskKind},
    tools::ToolDetails,
};

impl Client {
    pub async fn questions(
        &self,
        session: &str,
    ) -> Result<Vec<kyler_protocol::api::questions::QuestionRecord>> {
        valid_id(session)?;
        self.json(self.request(Method::GET, &format!("/v1/sessions/{session}/questions")))
            .await
    }
    pub async fn answer_question(
        &self,
        session: &str,
        id: &str,
        response: &kyler_protocol::api::questions::QuestionResponse,
    ) -> Result<kyler_protocol::api::questions::QuestionRecord> {
        valid_id(session)?;
        valid_id(id)?;
        self.json(
            self.request(
                Method::POST,
                &format!("/v1/sessions/{session}/questions/{id}/response"),
            )
            .json(response),
        )
        .await
    }

    pub async fn tasks(&self, session: &str) -> Result<Vec<TaskInfo>> {
        valid_id(session)?;
        self.json(self.request(Method::GET, &format!("/v1/sessions/{session}/tasks")))
            .await
    }
    pub async fn task(&self, session: &str, kind: TaskKind, id: &str) -> Result<TaskInfo> {
        valid_id(session)?;
        valid_id(id)?;
        self.json(self.request(
            Method::GET,
            &format!("/v1/sessions/{session}/tasks/{}/{id}", kind.path()),
        ))
        .await
    }
    pub async fn task_action(
        &self,
        session: &str,
        kind: TaskKind,
        id: &str,
        action: &TaskAction,
    ) -> Result<TaskInfo> {
        valid_id(session)?;
        valid_id(id)?;
        self.json(
            self.request(
                Method::POST,
                &format!("/v1/sessions/{session}/tasks/{}/{id}", kind.path()),
            )
            .json(action),
        )
        .await
    }

    /// Mutations are never automatically retried; callers retain a request ID for admission recovery.
    pub async fn processes(&self, session: &str) -> Result<Vec<ProcessInfo>> {
        valid_id(session)?;
        self.json(self.request(Method::GET, &format!("/v1/sessions/{session}/processes")))
            .await
    }
    pub async fn process(&self, session: &str, id: &str, cursor: u64) -> Result<ProcessInfo> {
        valid_id(session)?;
        valid_id(id)?;
        self.json(self.request(
            Method::GET,
            &format!("/v1/sessions/{session}/processes/{id}?cursor={cursor}"),
        ))
        .await
    }
    pub async fn tool_details(&self, session: &str, call: &str) -> Result<ToolDetails> {
        valid_id(session)?;
        let mut url =
            reqwest::Url::parse(&self.url(&format!("/v1/sessions/{session}/tool-calls/")))
                .map_err(|_| Error::invalid("Invalid URL"))?;
        url.path_segments_mut()
            .map_err(|_| Error::invalid("Invalid URL"))?
            .pop_if_empty()
            .push(call);
        self.json(self.request(Method::GET, url.path())).await
    }
}
