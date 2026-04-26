use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use thiserror::Error;

/// Single error type used across all handlers and middleware.
///
/// `IntoResponse` produces a uniform JSON body:
/// `{"error": {"code": "...", "message": "..."}}`.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("project forbidden")]
    ProjectForbidden,

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("too large: {0}")]
    TooLarge(String),

    #[error("db: {0}")]
    Db(#[from] sqlx::Error),

    #[error("internal: {0}")]
    Internal(#[from] anyhow::Error),
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, "NOT_FOUND", m.clone()),
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                "credentials required".into(),
            ),
            AppError::Forbidden(m) => (StatusCode::FORBIDDEN, "FORBIDDEN", m.clone()),
            AppError::ProjectForbidden => (
                StatusCode::FORBIDDEN,
                "PROJECT_FORBIDDEN",
                "no permission for this project".into(),
            ),
            AppError::InvalidInput(m) => (StatusCode::BAD_REQUEST, "INVALID_INPUT", m.clone()),
            AppError::Conflict(m) => (StatusCode::CONFLICT, "CONFLICT", m.clone()),
            AppError::TooLarge(m) => (StatusCode::PAYLOAD_TOO_LARGE, "TOO_LARGE", m.clone()),
            AppError::Db(e) => {
                tracing::error!(error = ?e, "db error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "INTERNAL",
                    "internal server error".into(),
                )
            }
            AppError::Internal(e) => {
                tracing::error!(error = ?e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "INTERNAL",
                    "internal server error".into(),
                )
            }
        };
        (
            status,
            Json(ErrorBody {
                error: ErrorDetail { code, message },
            }),
        )
            .into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
