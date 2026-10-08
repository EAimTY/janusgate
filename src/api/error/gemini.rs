//! Gemini error render: the wire payload types and the closed `render(&Error)` match for the
//! Gemini wire format (README §6.7).
//!
//! Gemini renders the official Status shape (google.aip.dev/193): `{"error": {code, message,
//! status}}` whose `code` is the numeric HTTP status and whose `status` is the canonical
//! google.rpc.Code-style name — `UNAUTHENTICATED`/`NOT_FOUND`/`INVALID_ARGUMENT`/`INTERNAL`/
//! `UNAVAILABLE` for the gateway's own failures, the canonical HTTP-to-code name where
//! google.rpc/code.proto defines one and otherwise the closest semantic member as an explicit
//! gateway extension (408 `DEADLINE_EXCEEDED`, 416 `OUT_OF_RANGE`, 502 `UNAVAILABLE`) for
//! mirrored upstream statuses, and `UNKNOWN` where the vocabulary has no member.

use super::Error;
use axum::http::StatusCode;
use serde::Serialize;

/// The Gemini error envelope.
#[derive(Debug, Serialize)]
pub(crate) struct GeminiErrorEnvelope {
    /// The rendered error object.
    error: GeminiErrorBody,
}

/// The Gemini error object.
#[derive(Debug, Serialize)]
struct GeminiErrorBody {
    /// HTTP status of the failure; the official Status schema types the JSON `code` as an
    /// int32 HTTP status code, not a code-name string.
    code: u16,

    /// Human-readable error message.
    message: String,

    /// Canonical google.rpc.Code-style status name (`NOT_FOUND`, `UNAUTHENTICATED`, ...).
    status: &'static str,
}

/// Renders the Gemini error body for one API failure. Compiler exhaustiveness over [`Error`] is
/// the invariant: a new variant forces this match to grow.
fn render_body(error: &Error) -> GeminiErrorBody {
    match error {
        Error::InvalidApiKey => GeminiErrorBody {
            code: StatusCode::UNAUTHORIZED.as_u16(),
            message: error.to_string(),
            status: "UNAUTHENTICATED",
        },

        Error::InvalidUrl { .. } | Error::ModelNotFound { .. } => GeminiErrorBody {
            code: StatusCode::NOT_FOUND.as_u16(),
            message: error.to_string(),
            status: "NOT_FOUND",
        },

        // The vocabulary has no member closer to a wrong method, an oversized body, or a
        // malformed request: the request itself is at fault, so INVALID_ARGUMENT carries all
        // three while the numeric `code` distinguishes their HTTP statuses.
        Error::MethodNotAllowed | Error::PayloadTooLarge | Error::MalformedRequest { .. } => {
            GeminiErrorBody {
                code: error.http_status().as_u16(),
                message: error.to_string(),
                status: "INVALID_ARGUMENT",
            }
        }

        Error::Upstream { status, .. } | Error::QuotaRejected { status, .. } => GeminiErrorBody {
            code: status.as_u16(),
            message: error.to_string(),
            status: canonical_status_name(*status),
        },

        Error::PoolUnavailable { .. } | Error::BoundAccountUnavailable { .. } => GeminiErrorBody {
            code: error.http_status().as_u16(),
            message: error.to_string(),
            status: "UNAVAILABLE",
        },

        Error::PoolQuotaExhausted { .. } | Error::BoundAccountQuotaExhausted { .. } => {
            GeminiErrorBody {
                code: error.http_status().as_u16(),
                message: error.to_string(),
                status: "RESOURCE_EXHAUSTED",
            }
        }

        Error::RoutingStateConflict { .. } => GeminiErrorBody {
            code: error.http_status().as_u16(),
            message: error.to_string(),
            status: "INVALID_ARGUMENT",
        },

        Error::UpstreamUnavailable { .. } => GeminiErrorBody {
            code: StatusCode::BAD_GATEWAY.as_u16(),
            message: error.to_string(),
            status: "UNAVAILABLE",
        },

        Error::Internal => GeminiErrorBody {
            code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            message: error.to_string(),
            status: "INTERNAL",
        },
    }
}

/// Maps a mirrored upstream HTTP status to its canonical google.rpc.Code-style name: the
/// canonical HTTP-to-code name (google.aip.dev/193, google.rpc/code.proto) where that mapping
/// defines a row — 400 `INVALID_ARGUMENT`, 401 `UNAUTHENTICATED`, 403 `PERMISSION_DENIED`, 404
/// `NOT_FOUND`, 409 `ABORTED`, 429 `RESOURCE_EXHAUSTED`, 499 `CANCELLED`, 500 `INTERNAL`, 501
/// `UNIMPLEMENTED`, 503 `UNAVAILABLE`, 504 `DEADLINE_EXCEEDED` — and otherwise the closest
/// semantic member as an explicit gateway extension for the statuses the canonical mapping has
/// no row for: 408 `DEADLINE_EXCEEDED`, 416 `OUT_OF_RANGE`, 502 `UNAVAILABLE`. A status outside
/// both degrades to `UNKNOWN`.
fn canonical_status_name(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 => "INVALID_ARGUMENT",
        401 => "UNAUTHENTICATED",
        403 => "PERMISSION_DENIED",
        404 => "NOT_FOUND",
        408 | 504 => "DEADLINE_EXCEEDED",
        409 => "ABORTED",
        416 => "OUT_OF_RANGE",
        429 => "RESOURCE_EXHAUSTED",
        499 => "CANCELLED",
        500 => "INTERNAL",
        501 => "UNIMPLEMENTED",
        502 | 503 => "UNAVAILABLE",
        _ => "UNKNOWN",
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
