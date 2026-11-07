//! Run admission, status and cancellation requests.
use super::{Client, valid_id};
use crate::error::Result;

use reqwest::Method;

use kyler_protocol::api::run::{CreateRunRequest, RunAccepted, RunInfo};

impl Client {
    pub async fn start_run(
        &self,
        session: &str,
        request: &CreateRunRequest,
    ) -> Result<RunAccepted> {
        valid_id(session)?;
        self.json(
            self.request(Method::POST, &format!("/v1/sessions/{session}/runs"))
                .json(request),
        )
        .await
    }
    pub async fn run(&self, id: &str) -> Result<RunInfo> {
        valid_id(id)?;
        self.json(self.request(Method::GET, &format!("/v1/runs/{id}")))
            .await
    }
    pub async fn cancel(&self, id: &str) -> Result<RunInfo> {
        valid_id(id)?;
        self.json(self.request(Method::POST, &format!("/v1/runs/{id}/cancel")))
            .await
    }
}
