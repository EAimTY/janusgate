//! Gemini error render: the wire payload types and the closed `render(&Error)` match for the
//! Gemini wire format (REFACTOR §3).
//!
//! Gemini renders `{"error": {code, message, status}}` with the canonical code vocabulary
//! (`UNAUTHENTICATED`/`NOT_FOUND`/`INVALID_ARGUMENT`/`INTERNAL`, mirrored numeric code with
//! `UNKNOWN` for upstream mirrors).

use crate::api::Error;
use axum::http::StatusCode;
use serde::Serialize;

/// The Gemini error envelope.
#[derive(Debug, Serialize)]
pub(crate) struct GeminiErrorEnvelope {
    /// The rendered error object.
    pub(crate) error: GeminiErrorBody,
}

/// The Gemini error object.
#[derive(Debug, Serialize)]
pub(crate) struct GeminiErrorBody {
    /// Canonical Gemini error code.
    pub(crate) code: &'static str,

    /// Human-readable error message.
    pub(crate) message: String,

    /// HTTP status text.
    pub(crate) status: String,
}

/// Renders the Gemini error body for one API failure. Compiler exhaustiveness over [`Error`] is
/// the invariant: a new variant forces this match to grow.
pub(crate) fn render_body(error: &Error) -> GeminiErrorBody {
    match error {
        Error::InvalidApiKey => GeminiErrorBody {
            code: "UNAUTHENTICATED",
            message: error.to_string(),
            status: status_text(StatusCode::UNAUTHORIZED),
        },

        Error::InvalidUrl { .. } | Error::ModelNotFound { .. } => GeminiErrorBody {
            code: "NOT_FOUND",
            message: error.to_string(),
            status: status_text(StatusCode::NOT_FOUND),
        },

        Error::MethodNotAllowed | Error::PayloadTooLarge | Error::MalformedRequest { .. } => {
            GeminiErrorBody {
                code: "INVALID_ARGUMENT",
                message: error.to_string(),
                status: status_text(error.http_status()),
            }
        }

        Error::Upstream { status, .. } => GeminiErrorBody {
            code: "UNKNOWN",
            message: error.to_string(),
            status: status_text(*status),
        },

        Error::UpstreamUnavailable { .. } => GeminiErrorBody {
            code: "UNKNOWN",
            message: error.to_string(),
            status: status_text(StatusCode::BAD_GATEWAY),
        },

        Error::Internal => GeminiErrorBody {
            code: "INTERNAL",
            message: error.to_string(),
            status: status_text(StatusCode::INTERNAL_SERVER_ERROR),
        },
    }
}

/// Renders the full Gemini error response: the HTTP status the variant pins beside its envelope.
pub(crate) fn render(error: &Error) -> (StatusCode, GeminiErrorEnvelope) {
    let status = error.http_status();

    (
        status,
        GeminiErrorEnvelope {
            error: render_body(error),
        },
    )
}

/// Renders the HTTP status text for the Gemini `status` field.
fn status_text(status: StatusCode) -> String {
    status.canonical_reason().unwrap_or("Unknown").to_string()
}
