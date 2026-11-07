//! Session attachment upload requests.
use super::{Client, valid_id};
use crate::error::{Error, Result};

use reqwest::Method;

use kyler_protocol::api::attachments::{AttachmentInfo, MAX_ATTACHMENT_BYTES};

impl Client {
    pub async fn upload_attachment(
        &self,
        session: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<AttachmentInfo> {
        valid_id(session)?;
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(Error::invalid("Attachment exceeds 20 MiB"));
        }
        self.json(
            self.request(Method::POST, &format!("/v1/sessions/{session}/attachments"))
                .query(&[("filename", name)])
                .header("content-type", "application/octet-stream")
                .body(bytes),
        )
        .await
    }
}
