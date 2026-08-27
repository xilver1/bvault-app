//! One error type for handlers, mapped to HTTP. Internal errors are logged with
//! detail but never leak it to the client.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use tracing::error;

pub enum GatewayError {
    NotFound,
    BadRequest(String),
    /// Missing/invalid credentials or session — always a generic 401 to the
    /// client (no distinction between "no such user" and "wrong password").
    Unauthorized,
    /// An internal failure with a message logged but not returned to the client.
    Internal(String),
    Meta(bvault_meta::Error),
    Jobs(bvault_jobs::Error),
}

impl From<bvault_meta::Error> for GatewayError {
    fn from(e: bvault_meta::Error) -> Self {
        Self::Meta(e)
    }
}

impl From<bvault_jobs::Error> for GatewayError {
    fn from(e: bvault_jobs::Error) -> Self {
        Self::Jobs(e)
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            GatewayError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            GatewayError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            GatewayError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            GatewayError::Internal(detail) => {
                error!(detail, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
            // A taken username is an expected, user-caused conflict, not a 500.
            GatewayError::Meta(bvault_meta::Error::UsernameTaken) => {
                (StatusCode::CONFLICT, "username already taken".to_string())
            }
            // Don't leak internals to the client; log them instead.
            GatewayError::Meta(e) => {
                error!(error = %e, "metadata error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
            GatewayError::Jobs(e) => {
                error!(error = %e, "queue error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub type ApiResult<T> = Result<T, GatewayError>;
