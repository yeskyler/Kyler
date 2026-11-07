//! Shared HTTP transport, authentication and bounded response handling.
mod attachments;
mod inspection;
mod mcp;
mod models;
mod permissions;
mod runs;
mod sessions;
mod skills;

use crate::error::{Error, ErrorKind, Result};

use std::time::Duration;

use futures::StreamExt;
use reqwest::{
    Method, Response,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::de::DeserializeOwned;

use kyler_config::ClientSettings;
use kyler_protocol::api::{ApiErrorBody, ServerInfo};

#[derive(Clone)]
pub struct Client {
    pub(crate) http: reqwest::Client,
    pub(crate) settings: ClientSettings,
}
impl Client {
    pub fn new(settings: ClientSettings) -> Result<Self> {
        settings
            .validate()
            .map_err(|_| Error::invalid("Invalid client connection settings."))?;
        let mut headers = HeaderMap::new();
        if !settings.access_token.is_empty() {
            let mut value =
                HeaderValue::from_str(&format!("Bearer {}", settings.access_token.expose()))
                    .map_err(|_| Error::invalid("Invalid client token."))?;
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
        }
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(settings.connect_timeout_seconds))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::transport())?;
        Ok(Self { http, settings })
    }

    pub fn settings(&self) -> &ClientSettings {
        &self.settings
    }
    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.settings.server_url.trim_end_matches('/'))
    }
    pub(super) fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, self.url(path))
            .timeout(Duration::from_secs(self.settings.request_timeout_seconds))
    }
    pub(super) async fn json<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T> {
        let response = request.send().await.map_err(|_| Error::transport())?;
        let response = self.checked(response).await?;
        let bytes = self.body(response).await?;
        serde_json::from_slice(&bytes)
            .map_err(|_| Error::protocol("The server returned invalid JSON."))
    }
    pub(crate) async fn body(&self, response: Response) -> Result<Vec<u8>> {
        if response
            .content_length()
            .is_some_and(|n| n > self.settings.max_response_bytes as u64)
        {
            return Err(Error::protocol(
                "Response exceeds the configured size limit.",
            ));
        }
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| Error::transport())?;
            if chunk.len() > self.settings.max_response_bytes.saturating_sub(body.len()) {
                return Err(Error::protocol(
                    "Response exceeds the configured size limit.",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
    pub(crate) async fn checked(&self, response: Response) -> Result<Response> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let body = self.body(response).await?;
        let error = serde_json::from_slice::<ApiErrorBody>(&body).unwrap_or(ApiErrorBody {
            code: format!("http_{status}"),
            message: "The server rejected the request.".into(),
        });
        Err(Error {
            kind: ErrorKind::Api,
            code: error.code,
            message: error.message,
            status: Some(status),
        })
    }
    pub async fn shutdown_instance(&self, instance_id: &str) -> Result<()> {
        valid_id(instance_id)?;
        let response = self
            .request(Method::POST, "/v1/service/shutdown")
            .header("x-kyler-instance-id", instance_id)
            .send()
            .await
            .map_err(|_| Error::transport())?;
        self.checked(response).await?;
        Ok(())
    }
    pub async fn info(&self) -> Result<ServerInfo> {
        self.json(self.request(Method::GET, "/v1/info")).await
    }
}
pub(crate) fn valid_id(id: &str) -> Result<()> {
    if uuid::Uuid::parse_str(id).is_ok_and(|v| v.to_string() == id) {
        Ok(())
    } else {
        Err(Error::invalid("Expected a canonical UUID."))
    }
}
