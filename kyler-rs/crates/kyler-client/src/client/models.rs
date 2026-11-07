//! Model catalog reload and session selection requests.
use super::{Client, valid_id};
use crate::error::Result;

use reqwest::Method;

use kyler_protocol::api::models::{ModelCatalog, ModelSelection, SessionModel};

impl Client {
    pub async fn models(&self, reload: bool) -> Result<ModelCatalog> {
        let (method, path) = if reload {
            (Method::POST, "/v1/models/reload")
        } else {
            (Method::GET, "/v1/models")
        };
        self.json(self.request(method, path)).await
    }
    pub async fn session_model(&self, session: &str) -> Result<SessionModel> {
        valid_id(session)?;
        self.json(self.request(Method::GET, &format!("/v1/sessions/{session}/model")))
            .await
    }
    pub async fn select_model(
        &self,
        session: &str,
        profile: Option<String>,
    ) -> Result<SessionModel> {
        valid_id(session)?;
        self.json(
            self.request(Method::PUT, &format!("/v1/sessions/{session}/model"))
                .json(&ModelSelection { profile }),
        )
        .await
    }
}
