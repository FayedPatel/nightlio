//! Unified API error type. Every handler returns `Result<_, ApiError>`;
//! the `IntoResponse` impl serializes the Flask-compatible error body
//! `{"error": "..."}` with the matching HTTP status.
//!
//! Mapping mirrors the Flask app (`api/app.py`):
//! - `ValueError` handler → [`ApiError::Validation`] → 400 with the message.
//! - unexpected/database/serialization failures → 500 with a generic
//!   message (details go to the log, never to the client).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// Client-caused bad input; equivalent of the Flask `ValueError`
    /// handler. The message is shown to the client verbatim.
    #[error("{0}")]
    Validation(String),

    /// Authentication required or failed.
    #[error("{0}")]
    Unauthorized(String),

    /// Resource does not exist (or is not owned by the caller).
    #[error("{0}")]
    NotFound(String),

    /// SQLite failure — internal, message never leaves the server.
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// Connection-pool failure — internal.
    #[error("connection pool error: {0}")]
    Pool(#[from] r2d2::Error),

    /// JSON (de)serialization failure — internal.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Anything else unexpected — internal.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl ApiError {
    /// Convenience constructor for validation failures.
    pub fn validation(message: impl Into<String>) -> Self {
        ApiError::Validation(message.into())
    }
}

/// Data-layer failure → the matching internal variant (all map to a generic
/// 500). Mirrors the per-route `db_err` helpers so call sites checking a
/// connection out of [`crate::db::DbHandle`] can use `?` directly.
impl From<crate::db::DatabaseError> for ApiError {
    fn from(error: crate::db::DatabaseError) -> Self {
        match error {
            crate::db::DatabaseError::Sqlite(inner) => ApiError::Database(inner),
            crate::db::DatabaseError::Pool(inner) => ApiError::Pool(inner),
            crate::db::DatabaseError::Message(message) => {
                ApiError::Internal(anyhow::anyhow!(message))
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ApiError::Validation(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            ApiError::Database(_)
            | ApiError::Pool(_)
            | ApiError::Serialization(_)
            | ApiError::Internal(_) => {
                // Log the real cause server-side; the client only ever sees
                // a generic message.
                tracing::error!(error = %self, "internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json body")
    }

    #[tokio::test]
    async fn validation_maps_to_400_with_message() {
        let response = ApiError::validation("Invalid mood value").into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Invalid mood value" })
        );
    }

    #[tokio::test]
    async fn rusqlite_error_maps_to_500_generic_body() {
        let err: ApiError = rusqlite::Error::QueryReturnedNoRows.into();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Internal server error" })
        );
    }

    #[tokio::test]
    async fn serde_error_maps_to_500_generic_body() {
        let serde_err = serde_json::from_str::<serde_json::Value>("{not json").unwrap_err();
        let err: ApiError = serde_err.into();
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Internal server error" })
        );
    }
}
