//! Anthropic error render: the wire payload types and the closed `render(&Error)` match for
//! the Anthropic wire format (README §6.7).
//!
//! Anthropic renders `{"type": "error", "error": {type, message}}`, mapping each rendered HTTP
//! status onto the official status↔type table: 400/405 `invalid_request_error`, 401
//! `authentication_error`, 402 `billing_error`, 403 `permission_error`, 404 `not_found_error`,
//! 409 `conflict_error`, 413 `request_too_large`, 429 `rate_limit_error`, 504 `timeout_error`,
//! 529 `overloaded_error`, and the 500-class failures `api_error`. Mirrored 503 also renders
//! `overloaded_error` — the closest semantic member as an explicit gateway extension, the same
//! pattern the Gemini surface documents, because the official table has no 503 row.

use super::Error;
use axum::http::StatusCode;
use serde::Serialize;

/// The Anthropic error envelope.
#[derive(Debug, Serialize)]
pub(in crate::api) struct AnthropicErrorEnvelope {
    /// Always `error`.
    r#type: &'static str,

    /// The rendered error object.
    error: AnthropicErrorBody,
}

/// The Anthropic error object.
#[derive(Debug, Serialize)]
pub(crate) struct AnthropicErrorBody {
    /// Anthropic error type.
    r#type: &'static str,

    /// Human-readable error message.
    message: String,
}

/// Maps one mirrored upstream status onto the official error-`type` row it renders: a mirrored
/// status chooses its own table row, mirrored 503 renders `overloaded_error` as the closest
/// semantic member as an explicit gateway extension where the official table has no row, and a
/// status with neither a row nor a closer member — like a mirrored 502 — degrades to
/// `api_error`, the gateway's generic failure row.
fn mirrored_error_type(status: u16) -> &'static str {
    match status {
        400 | 405 => "invalid_request_error",

        401 => "authentication_error",

        402 => "billing_error",

        403 => "permission_error",

        404 => "not_found_error",

        409 => "conflict_error",

        413 => "request_too_large",

        429 => "rate_limit_error",

        503 | 529 => "overloaded_error",

        504 => "timeout_error",

        _ => "api_error",
    }
}

/// Renders the Anthropic error body for one API failure. Compiler exhaustiveness over [`Error`] is
/// the invariant: a new variant forces this match to grow.
pub(crate) fn render_body(error: &Error) -> AnthropicErrorBody {
    let render = |r#type: &'static str| AnthropicErrorBody {
        r#type,
        message: error.to_string(),
    };

    match error {
        Error::InvalidApiKey => render("authentication_error"),

        Error::InvalidUrl { .. } | Error::ModelNotFound { .. } => render("not_found_error"),

        Error::MethodNotAllowed
        | Error::MalformedRequest { .. }
        | Error::RoutingStateConflict { .. } => render("invalid_request_error"),

        Error::PayloadTooLarge => render("request_too_large"),

        Error::PoolQuotaExhausted { .. } | Error::BoundAccountQuotaExhausted { .. } => {
            render("rate_limit_error")
        }

        Error::Upstream { status, .. } | Error::QuotaRejected { status, .. } => {
            render(mirrored_error_type(status.as_u16()))
        }

        // The official table has no 502 row and `api_error` is the gateway's own failure row:
        // its own 502 upstream-unavailable failure and its 500 internal failure render there.
        Error::PoolUnavailable { .. }
        | Error::BoundAccountUnavailable { .. }
        | Error::UpstreamUnavailable { .. }
        | Error::Internal => render("api_error"),
    }
}

/// Renders the full Anthropic error response: the HTTP status the variant pins beside its
/// envelope.
pub(in crate::api) fn render(error: &Error) -> (StatusCode, AnthropicErrorEnvelope) {
    let status = error.http_status();

    (
        status,
        AnthropicErrorEnvelope {
            r#type: "error",
            error: render_body(error),
        },
    )
}
