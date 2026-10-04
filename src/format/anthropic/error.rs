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

use crate::api::Error;
use axum::http::StatusCode;
use serde::Serialize;

/// The Anthropic error envelope.
#[derive(Debug, Serialize)]
pub(crate) struct AnthropicErrorEnvelope {
    /// Always `error`.
    pub(crate) r#type: &'static str,

    /// The rendered error object.
    pub(crate) error: AnthropicErrorBody,
}

/// The Anthropic error object.
#[derive(Debug, Serialize)]
pub(crate) struct AnthropicErrorBody {
    /// Anthropic error type.
    pub(crate) r#type: &'static str,

    /// Human-readable error message.
    pub(crate) message: String,
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
pub(crate) fn render(error: &Error) -> (StatusCode, AnthropicErrorEnvelope) {
    let status = error.http_status();

    (
        status,
        AnthropicErrorEnvelope {
            r#type: "error",
            error: render_body(error),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::at;
    use serde_json::{Value, json};

    /// Serializes one rendered error body for inspection.
    ///
    /// # Panics
    ///
    /// Panics if the rendered body fails to serialize.
    fn body_json(error: &Error) -> Value {
        #[expect(
            clippy::expect_used,
            reason = "the rendered body is under test; a failing render is the failure"
        )]
        serde_json::to_value(render_body(error)).expect("the error body serializes")
    }

    /// Builds one mirrored upstream failure at the given status.
    fn mirrored(status: StatusCode) -> Error {
        Error::Upstream {
            status,
            message: "upstream excerpt".to_string(),
        }
    }

    /// Verifies the gateway's own rows against the official status↔type table: the 413 body
    /// limit renders `request_too_large`, the 400/401/404/405 rows keep their documented types,
    /// and the gateway's own 500-class failures (its internal row and its 502 upstream-
    /// unavailable row — the official table has no 502 row) render `api_error`.
    ///
    /// # Panics
    ///
    /// Panics if any row renders outside its official type.
    #[test]
    fn renders_the_official_type_for_each_own_row() {
        let cases: Vec<(Error, &str)> = vec![
            (Error::InvalidApiKey, "authentication_error"),
            (
                Error::InvalidUrl {
                    method: "GET".to_string(),
                    path: "/nope".to_string(),
                },
                "not_found_error",
            ),
            (Error::MethodNotAllowed, "invalid_request_error"),
            (Error::PayloadTooLarge, "request_too_large"),
            (
                Error::MalformedRequest {
                    param: Some("model".to_string()),
                    message: "bad model".to_string(),
                },
                "invalid_request_error",
            ),
            (
                Error::ModelNotFound {
                    model: "claude-test".to_string(),
                },
                "not_found_error",
            ),
            (
                Error::UpstreamUnavailable {
                    message: "connection refused".to_string(),
                },
                "api_error",
            ),
            (Error::Internal, "api_error"),
        ];

        for (error, r#type) in cases {
            assert_eq!(
                at(&body_json(&error), "/type"),
                &json!(r#type),
                "type mismatch for {error}"
            );
        }
    }

    /// Maps every mirrored upstream status to its row — 400 `invalid_request_error`, 401
    /// `authentication_error`, 402 `billing_error`, 403 `permission_error`, 404
    /// `not_found_error`, 409 `conflict_error`, 429 `rate_limit_error`, 529 `overloaded_error`,
    /// 504 `timeout_error` — with mirrored 503 rendering `overloaded_error` as the
    /// closest-semantic-member gateway extension where the official table has no row.
    ///
    /// # Panics
    ///
    /// Panics if a mirrored status renders outside its official row.
    #[test]
    fn maps_mirrored_upstream_statuses_to_their_table_rows() {
        #[expect(
            clippy::expect_used,
            reason = "529 is a valid status code; the fixture is constructed to parse"
        )]
        let overloaded = StatusCode::from_u16(529).expect("529 is a valid status code");

        let cases: Vec<(StatusCode, &str)> = vec![
            (StatusCode::BAD_REQUEST, "invalid_request_error"),
            (StatusCode::UNAUTHORIZED, "authentication_error"),
            (StatusCode::PAYMENT_REQUIRED, "billing_error"),
            (StatusCode::FORBIDDEN, "permission_error"),
            (StatusCode::NOT_FOUND, "not_found_error"),
            (StatusCode::CONFLICT, "conflict_error"),
            (StatusCode::TOO_MANY_REQUESTS, "rate_limit_error"),
            (StatusCode::SERVICE_UNAVAILABLE, "overloaded_error"),
            (overloaded, "overloaded_error"),
            (StatusCode::GATEWAY_TIMEOUT, "timeout_error"),
        ];

        for (status, r#type) in cases {
            assert_eq!(
                at(&body_json(&mirrored(status)), "/type"),
                &json!(r#type),
                "type mismatch for mirrored status {status}"
            );
        }
    }
}
