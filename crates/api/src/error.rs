//! API error type — maps store/internal failures to HTTP responses.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use lighttrack_store::StoreError;

pub(crate) struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, m: impl Into<String>) -> Self {
        Self {
            status,
            message: m.into(),
        }
    }
    pub(crate) fn internal(m: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, m)
    }
    pub(crate) fn bad_request(m: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, m)
    }
    pub(crate) fn unauthorized(m: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, m)
    }
    pub(crate) fn forbidden(m: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, m)
    }
    pub(crate) fn not_found(m: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, m)
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        ApiError::internal(e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(serde_json::json!({ "error": self.message }))).into_response()
    }
}
