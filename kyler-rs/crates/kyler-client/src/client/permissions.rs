//! Session permission selection and exact-call approval requests.
use super::{Client, valid_id};
use crate::error::Result;

use reqwest::Method;

use kyler_protocol::api::permissions::{
    ApprovalRecord, ApprovalResponse, PermissionSelection, PermissionState,
};

impl Client {
    pub async fn permissions(&self, session: &str) -> Result<PermissionState> {
        valid_id(session)?;
        self.json(self.request(Method::GET, &format!("/v1/sessions/{session}/permissions")))
            .await
    }
    pub async fn select_permissions(
        &self,
        session: &str,
        selection: &PermissionSelection,
    ) -> Result<PermissionState> {
        valid_id(session)?;
        self.json(
            self.request(Method::PUT, &format!("/v1/sessions/{session}/permissions"))
                .json(selection),
        )
        .await
    }
    pub async fn approvals(&self, session: &str) -> Result<Vec<ApprovalRecord>> {
        valid_id(session)?;
        self.json(self.request(Method::GET, &format!("/v1/sessions/{session}/approvals")))
            .await
    }
    pub async fn decide_approval(
        &self,
        id: &str,
        response: &ApprovalResponse,
    ) -> Result<ApprovalRecord> {
        valid_id(id)?;
        self.json(
            self.request(Method::POST, &format!("/v1/approvals/{id}/decision"))
                .json(response),
        )
        .await
    }
}
