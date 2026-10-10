//! OpenAI error render: the wire payload types and the closed `render(&Error)` match for the
//! OpenAI wire format (`docs/reference/inference.md`).
//!
//! The OpenAI render keeps the existing `type`/`code`/`param` rows verbatim; the new contract rows
//! (method, body limit, fixed-message internal) extend the same table.

use super::Error;
use axum::http::StatusCode;
use serde::Serialize;

/// The OpenAI error object: the shape every OpenAI outward failure renders as, whether in a JSON
/// error envelope, inside a streamed Chat Completions error frame, or as the `error` of a failed
/// Responses response object.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct OpenAiErrorBody {
    /// Human-readable error message.
    message: String,

    /// OpenAI `type` field.
    r#type: &'static str,

    /// OpenAI `param` field, when the failure names a request parameter.
    param: Option<String>,

    /// OpenAI `code` field, when the failure has a stable code.
    code: Option<String>,
}

/// The JSON error envelope `{"error": {...}}` shared by the JSON error response and the streamed
/// Chat Completions error frame, so both render the same envelope from the same type.
#[derive(Debug, Serialize)]
pub(crate) struct OpenAiErrorEnvelope {
    /// The rendered error object.
    pub(crate) error: OpenAiErrorBody,
}

/// Renders the OpenAI error object for one API failure: the classified row beside the
/// human-readable message. Compiler exhaustiveness over [`Error`] is the invariant: a new variant
/// forces this match to grow.
pub(crate) fn render_body(error: &Error) -> OpenAiErrorBody {
    match error {
        Error::InvalidApiKey => OpenAiErrorBody {
            message: error.to_string(),
            r#type: "invalid_request_error",
            param: None,
            code: Some("invalid_api_key".to_string()),
        },

        Error::InvalidUrl { .. } => OpenAiErrorBody {
            message: error.to_string(),
            r#type: "invalid_request_error",
            param: None,
            code: None,
        },

        Error::MethodNotAllowed | Error::PayloadTooLarge => OpenAiErrorBody {
            message: error.to_string(),
            r#type: "invalid_request_error",
            param: None,
            code: None,
        },

        Error::MalformedRequest { param, .. } => OpenAiErrorBody {
            message: error.to_string(),
            r#type: "invalid_request_error",
            param: param.clone(),
            code: None,
        },

        Error::ModelNotFound { .. } => OpenAiErrorBody {
            message: error.to_string(),
            r#type: "invalid_request_error",
            param: Some("model".to_string()),
            code: Some("model_not_found".to_string()),
        },

        Error::PoolUnavailable { .. } => OpenAiErrorBody {
            message: error.to_string(),
            r#type: "api_error",
            param: None,
            code: Some("pool_unavailable".to_string()),
        },

        Error::PoolQuotaExhausted { .. } => OpenAiErrorBody {
            message: error.to_string(),
            r#type: "rate_limit_error",
            param: None,
            code: Some("pool_quota_exhausted".to_string()),
        },

        Error::BoundAccountUnavailable { .. } => OpenAiErrorBody {
            message: error.to_string(),
            r#type: "api_error",
            param: None,
            code: Some("bound_account_unavailable".into()),
        },
        Error::BoundAccountQuotaExhausted { .. } => OpenAiErrorBody {
            message: error.to_string(),
            r#type: "rate_limit_error",
            param: None,
            code: Some("bound_account_quota_exhausted".into()),
        },
        Error::RoutingStateConflict { .. } => OpenAiErrorBody {
            message: error.to_string(),
            r#type: "invalid_request_error",
            param: None,
            code: Some("routing_state_conflict".into()),
        },

        Error::Upstream { .. }
        | Error::QuotaRejected { .. }
        | Error::UpstreamUnavailable { .. } => OpenAiErrorBody {
            message: error.to_string(),
            r#type: "api_error",
            param: None,
            code: Some("upstream_error".to_string()),
        },

        Error::Internal => OpenAiErrorBody {
            message: error.to_string(),
            r#type: "api_error",
            param: None,
            code: Some("internal".to_string()),
        },
    }
}

/// Renders the full OpenAI error response: the HTTP status the variant pins beside its envelope.
pub(in crate::api) fn render(error: &Error) -> (StatusCode, OpenAiErrorEnvelope) {
    let status = error.http_status();

    (
        status,
        OpenAiErrorEnvelope {
            error: render_body(error),
        },
    )
}
