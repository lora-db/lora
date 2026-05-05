use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use lora_database::{LoraError, LoraErrorCode};
use serde::Serialize;

/// Structured error body returned by every fallible HTTP endpoint.
///
/// Wire shape:
/// ```json
/// { "error": { "code": "LORA_PARSE", "message": "...", "category": "client" } }
/// ```
///
/// `code` is a stable wire string from the [`LoraErrorCode`] catalog and
/// is the field bindings / dashboards / tests should match on. `message`
/// is human-friendly and may be reworded between releases. `category` is
/// `"client"` for caller mistakes (4xx) and `"server"` for engine
/// failures (5xx).
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
    pub category: &'static str,
}

impl ErrorResponse {
    fn from_lora(err: &LoraError) -> Self {
        Self {
            error: ErrorBody {
                code: err.code().as_str(),
                message: err.message().to_string(),
                category: err.category().as_str(),
            },
        }
    }

    /// Build an ad-hoc error response for cases that never reach the
    /// engine (e.g. config-level argument validation in a handler).
    pub(crate) fn from_parts(code: LoraErrorCode, message: impl Into<String>) -> Self {
        Self {
            error: ErrorBody {
                code: code.as_str(),
                message: message.into(),
                category: code.category().as_str(),
            },
        }
    }
}

/// Map a [`LoraError`] to its HTTP status code.
///
/// Server-category errors collapse to 500, with one refinement:
/// `WalPoisoned` → 503 because the engine cannot accept further writes
/// until an operator restarts from snapshot + WAL.
///
/// Client-category errors collapse to 400, with refinements that match
/// standard HTTP semantics:
/// * `Timeout` → 408 (cooperative-deadline expired)
/// * `NotFound` → 404 (named entity does not exist)
/// * `InvalidParams` / `InvalidVector` → 422 (well-formed request,
///   semantically invalid value)
/// * `ConstraintViolation` → 409 (action conflicts with current state)
fn status_for(err: &LoraError) -> StatusCode {
    match err.code() {
        // Server-category
        LoraErrorCode::WalPoisoned => StatusCode::SERVICE_UNAVAILABLE,
        LoraErrorCode::Io
        | LoraErrorCode::WalCorruption
        | LoraErrorCode::SnapshotCodec
        | LoraErrorCode::SnapshotCrypto
        | LoraErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        // Client-category
        LoraErrorCode::Timeout => StatusCode::REQUEST_TIMEOUT,
        LoraErrorCode::NotFound => StatusCode::NOT_FOUND,
        LoraErrorCode::InvalidParams | LoraErrorCode::InvalidVector => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        LoraErrorCode::ConstraintViolation => StatusCode::CONFLICT,
        LoraErrorCode::Parse
        | LoraErrorCode::Semantic
        | LoraErrorCode::ReadOnlyViolation
        | LoraErrorCode::DatabaseName
        | LoraErrorCode::Config => StatusCode::BAD_REQUEST,
    }
}

pub(crate) fn lora_error_response(err: impl Into<LoraError>) -> Response {
    let lora = err.into();
    let status = status_for(&lora);
    (status, Json(ErrorResponse::from_lora(&lora))).into_response()
}
