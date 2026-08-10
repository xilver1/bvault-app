//! Handler error type mapped to HTTP; internals logged, not leaked.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use tracing::error;

pub enum ExportError {
    NotFound,
    BadRequest(String),
    Meta(rekordbox_meta::Error),
    Build(rekordbox_export::Error),
    Internal(String),
}

impl From<rekordbox_meta::Error> for ExportError {
    fn from(e: rekordbox_meta::Error) -> Self {
        Self::Meta(e)
    }
}

impl From<rekordbox_export::Error> for ExportError {
    fn from(e: rekordbox_export::Error) -> Self {
        Self::Build(e)
    }
}

impl IntoResponse for ExportError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ExportError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            ExportError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ExportError::Meta(e) => {
                error!(error = %e, "metadata error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
            }
            ExportError::Build(e) => {
                error!(error = %e, "export build failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "build failed".to_string())
            }
            ExportError::Internal(m) => {
                error!(error = %m, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ExportError>;