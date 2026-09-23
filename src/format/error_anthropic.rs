//! Anthropic error render: the wire payload types and the closed `render(&Error)` match for
//! the Anthropic wire format (REFACTOR §3).
//!
//! Anthropic renders `{"type": "error", "error": {type, message}}` with the `type` vocabulary
//! (`authentication_error`, `not_found_error`, `invalid_request_error`, `api_error`).

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

/// Renders the Anthropic error body for one API failure. Compiler exhaustiveness over [`Error`] is
/// the invariant: a new variant forces this match to grow.
pub(crate) fn render_body(error: &Error) -> AnthropicErrorBody {
    match error {
        Error::InvalidApiKey => AnthropicErrorBody {
            r#type: "authentication_error",
            message: error.to_string(),
        },

        Error::InvalidUrl { .. } | Error::ModelNotFound { .. } => AnthropicErrorBody {
            r#type: "not_found_error",
            message: error.to_string(),
        },

        Error::MethodNotAllowed | Error::PayloadTooLarge | Error::MalformedRequest { .. } => {
            AnthropicErrorBody {
                r#type: "invalid_request_error",
                message: error.to_string(),
            }
        }

        Error::Upstream { .. } | Error::UpstreamUnavailable { .. } | Error::Internal => {
            AnthropicErrorBody {
                r#type: "api_error",
                message: error.to_string(),
            }
        }
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
