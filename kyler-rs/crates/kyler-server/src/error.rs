use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

use kyler_protocol::api::ApiErrorBody;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub body: ApiErrorBody,
}
pub type ApiResult<T> = Result<T, ApiError>;
impl ApiError {
    pub fn new(status: StatusCode, code: &str, message: &str) -> Self {
        Self {
            status,
            body: ApiErrorBody {
                code: code.into(),
                message: message.into(),
            },
        }
    }
    pub fn bad(message: &str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
    pub fn missing() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "The requested resource was not found.",
        )
    }
    pub fn conflict() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "session_busy",
            "The session already has an active execution.",
        )
    }
    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage_error",
            "The operation could not be persisted or restored.",
        )
    }
    pub fn stopping() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "server_stopping",
            "The server is shutting down.",
        )
    }
    pub fn storage(error: anyhow::Error) -> Self {
        if error.to_string() == "session not found" {
            Self::missing()
        } else {
            tracing::error!("server storage operation failed");
            Self::internal()
        }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}
pub fn valid_id(id: &str) -> ApiResult<()> {
    if uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id) {
        Ok(())
    } else {
        Err(ApiError::bad("A canonical UUID is required."))
    }
}
