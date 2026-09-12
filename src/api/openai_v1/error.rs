//! The outward format surface's own error domain: rendering OpenAI error
//! bodies (README §6.7).
//!
//! Domain failures arrive through [`From`](std::convert::From) conversions
//! and are classified here onto their contract rows: the surface owns the
//! status, the OpenAI `type`, `code`, and `param`, while the human message
//! rides on the domain error's [`Display`](std::fmt::Display).

use crate::{
    gateway::Error as GatewayError,
    ir::InvalidRequest,
    provider::{oauth, storage},
};
use axum::{
    body::Body,
    http::{StatusCode, header},
    response::Response,
};
use log::{debug, error};

/// Status for the upstream failures whose foreign status is never mirrored.
const UPSTREAM_UNAVAILABLE_STATUS: u16 = 502;

/// An error surfaced to an outward API client.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The request carried a missing or invalid API key.
    #[error("Incorrect API key provided.")]
    InvalidApiKey,

    /// The request path is not routed; the surface fallback.
    #[error("Invalid URL ({method} {path})")]
    InvalidUrl {
        /// Request method named in the message.
        method: String,

        /// Request path named in the message.
        path: String,
    },

    /// The client's request failed validation before or while preparing the
    /// upstream call.
    #[error(transparent)]
    InvalidRequest(#[from] InvalidRequest),

    /// The completion pipeline failed.
    #[error(transparent)]
    Gateway(#[from] GatewayError),

    /// Storage failed outside the pipeline (model listing).
    #[error(transparent)]
    Storage(#[from] storage::Error),
}

/// One rendered contract row: everything the HTTP status and the OpenAI
/// error body need besides the message.
struct Row {
    /// HTTP status the surface answers with.
    status: u16,

    /// OpenAI `type` field.
    error_type: &'static str,

    /// OpenAI `param` field.
    param: Option<String>,

    /// OpenAI `code` field.
    code: Option<String>,
}

impl Error {
    /// Classifies the error onto its contract row. The classification keys
    /// on the failing domain module, so a token-endpoint rejection always
    /// pins 502 even though it carries a foreign status, while the
    /// completion upstream's status is mirrored verbatim (README §6.7).
    fn row(&self) -> Row {
        match self {
            Self::InvalidApiKey => Row {
                status: 401,
                error_type: "invalid_request_error",
                param: None,
                code: Some("invalid_api_key".to_string()),
            },

            Self::InvalidUrl { .. } => Row {
                status: 404,
                error_type: "invalid_request_error",
                param: None,
                code: None,
            },

            Self::InvalidRequest(error) => Row {
                status: 400,
                error_type: "invalid_request_error",
                param: error.param.clone(),
                code: None,
            },

            Self::Gateway(error) => match error {
                GatewayError::InvalidRequest(error) => Row {
                    status: 400,
                    error_type: "invalid_request_error",
                    param: error.param.clone(),
                    code: None,
                },

                GatewayError::ModelNotFound { .. } => Row {
                    status: 404,
                    error_type: "invalid_request_error",
                    param: Some("model".to_string()),
                    code: Some("model_not_found".to_string()),
                },

                GatewayError::Upstream { status, .. } => Row {
                    status: *status,
                    error_type: "api_error",
                    param: None,
                    code: Some("upstream_error".to_string()),
                },

                GatewayError::Unavailable { .. } => Row {
                    status: UPSTREAM_UNAVAILABLE_STATUS,
                    error_type: "api_error",
                    param: None,
                    code: Some("upstream_error".to_string()),
                },

                GatewayError::OAuth(oauth_error) => match oauth_error {
                    oauth::Error::NotFound { .. } => Row {
                        status: 404,
                        error_type: "invalid_request_error",
                        param: None,
                        code: Some("not_found".to_string()),
                    },

                    _ => Row {
                        status: UPSTREAM_UNAVAILABLE_STATUS,
                        error_type: "api_error",
                        param: None,
                        code: Some("upstream_error".to_string()),
                    },
                },

                GatewayError::Storage(_) | GatewayError::Internal { .. } => Row {
                    status: 500,
                    error_type: "api_error",
                    param: None,
                    code: Some("internal".to_string()),
                },
            },

            Self::Storage(_) => Row {
                status: 500,
                error_type: "api_error",
                param: None,
                code: Some("internal".to_string()),
            },
        }
    }

    /// Whether the error is the calling client's own fault — a malformed or
    /// unsupported request, a failed authentication, or an unknown model —
    /// as opposed to a gateway or upstream failure.
    pub(crate) fn is_client_fault(&self) -> bool {
        match self {
            Self::InvalidApiKey | Self::InvalidUrl { .. } | Self::InvalidRequest(_) => true,

            Self::Gateway(error) => matches!(
                error,
                GatewayError::InvalidRequest(_)
                    | GatewayError::ModelNotFound { .. }
                    | GatewayError::OAuth(oauth::Error::NotFound { .. })
            ),

            _ => false,
        }
    }

    /// Renders the OpenAI JSON error structure.
    pub(crate) fn to_openai_json(&self) -> serde_json::Value {
        let row = self.row();

        serde_json::json!({
            "error": {
                "message": self.to_string(),
                "type": row.error_type,
                "param": row.param,
                "code": row.code,
            }
        })
    }

    /// Renders the error into an HTTP response.
    pub(crate) fn into_response(self) -> Response {
        json_response(self.row().status, self.to_openai_json())
    }
}

/// Builds a JSON response with the given status code.
///
/// The outward surface's own response builder, kept separate from the
/// management one so each router owns its rendering.
///
/// # Panics
///
/// Panics if the static response parts fail to build, which cannot occur.
pub(crate) fn json_response(status: u16, body: serde_json::Value) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    #[expect(clippy::expect_used, reason = "JSON responses have known-valid parts")]
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("static response parts are always valid")
}

/// Logs one [`Error`] exactly once, at the response boundary about to render
/// it to its caller: a client fault is recorded at debug level (the rendered
/// response already informs the caller), while a gateway or upstream failure
/// is recorded at error level. `operation` names the request the error
/// belongs to, with whatever identifying context the boundary holds (such as
/// the model or provider name).
pub(crate) fn log_error(operation: &str, error: &Error) {
    if error.is_client_fault() {
        debug!("{operation} rejected: {error}");
    } else {
        error!("{operation} failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{format::PayloadError, provider::storage::Error as StorageError};

    /// Verifies that every domain failure classifies onto its row of the
    /// outward error table (README §6.7 and §4): the derived status, the
    /// OpenAI `type`, and the row's `code`, `param`, and message wording.
    ///
    /// # Panics
    ///
    /// Panics if any rendered row diverges from its contract.
    #[test]
    fn renders_outward_error_table_rows() {
        let cases = [
            (
                Error::InvalidApiKey,
                401,
                serde_json::json!({
                    "message": "Incorrect API key provided.",
                    "type": "invalid_request_error",
                    "param": serde_json::Value::Null,
                    "code": "invalid_api_key",
                }),
            ),
            (
                Error::InvalidUrl {
                    method: "GET".to_string(),
                    path: "/nope".to_string(),
                },
                404,
                serde_json::json!({
                    "message": "Invalid URL (GET /nope)",
                    "type": "invalid_request_error",
                    "param": serde_json::Value::Null,
                    "code": serde_json::Value::Null,
                }),
            ),
            (
                Error::from(InvalidRequest::field("model", "bad model")),
                400,
                serde_json::json!({
                    "message": "bad model",
                    "type": "invalid_request_error",
                    "param": "model",
                    "code": serde_json::Value::Null,
                }),
            ),
            (
                Error::from(GatewayError::ModelNotFound {
                    model: "gpt-4o".to_string(),
                }),
                404,
                serde_json::json!({
                    "message": "The model `gpt-4o` does not exist or you do not have access to it.",
                    "type": "invalid_request_error",
                    "param": "model",
                    "code": "model_not_found",
                }),
            ),
            (
                Error::from(GatewayError::Upstream {
                    status: 429,
                    detail: "rate limited".to_string(),
                }),
                429,
                serde_json::json!({
                    "message": "Upstream request failed: rate limited",
                    "type": "api_error",
                    "param": serde_json::Value::Null,
                    "code": "upstream_error",
                }),
            ),
            (
                Error::from(GatewayError::from(PayloadError::new("connection refused"))),
                UPSTREAM_UNAVAILABLE_STATUS,
                serde_json::json!({
                    "message": "Upstream request failed: connection refused",
                    "type": "api_error",
                    "param": serde_json::Value::Null,
                    "code": "upstream_error",
                }),
            ),
            (
                Error::from(GatewayError::OAuth(oauth::Error::Rejected {
                    status: 429,
                    detail: "rate limited".to_string(),
                })),
                UPSTREAM_UNAVAILABLE_STATUS,
                serde_json::json!({
                    "message": "Upstream request failed with status 429: rate limited",
                    "type": "api_error",
                    "param": serde_json::Value::Null,
                    "code": "upstream_error",
                }),
            ),
            (
                Error::from(GatewayError::OAuth(oauth::Error::NotFound {
                    message: "provider vanished during token refresh".to_string(),
                })),
                404,
                serde_json::json!({
                    "message": "provider vanished during token refresh",
                    "type": "invalid_request_error",
                    "param": serde_json::Value::Null,
                    "code": "not_found",
                }),
            ),
            (
                Error::from(GatewayError::Internal {
                    message: "storage drift".to_string(),
                }),
                500,
                serde_json::json!({
                    "message": "storage drift",
                    "type": "api_error",
                    "param": serde_json::Value::Null,
                    "code": "internal",
                }),
            ),
            (
                Error::from(StorageError::Drift {
                    message: "drift".to_string(),
                }),
                500,
                serde_json::json!({
                    "message": "drift",
                    "type": "api_error",
                    "param": serde_json::Value::Null,
                    "code": "internal",
                }),
            ),
        ];

        for (error, status, body) in cases {
            assert_eq!(error.row().status, status);

            let rendered = error.to_openai_json();

            assert_eq!(rendered["error"], body, "row mismatch for {rendered}");
        }
    }

    /// Verifies that the fault-party classification covers exactly the
    /// client-fault rows, so the debug-versus-error log policy (README §8)
    /// keys on the party alone.
    ///
    /// # Panics
    ///
    /// Panics if a variant is classified with the wrong party.
    #[test]
    fn classifies_fault_parties() {
        let client_faults = [
            Error::InvalidApiKey,
            Error::InvalidUrl {
                method: "GET".to_string(),
                path: "/nope".to_string(),
            },
            Error::from(InvalidRequest::field("model", "bad model")),
            Error::from(GatewayError::ModelNotFound {
                model: "gpt-4o".to_string(),
            }),
            Error::from(GatewayError::OAuth(oauth::Error::NotFound {
                message: "vanished".to_string(),
            })),
        ];

        for error in client_faults {
            assert!(error.is_client_fault(), "{error} is a client fault");
        }

        let other_faults = [
            Error::from(GatewayError::Upstream {
                status: 429,
                detail: "rate limited".to_string(),
            }),
            Error::from(GatewayError::Unavailable {
                detail: "connection refused".to_string(),
            }),
            Error::from(GatewayError::OAuth(oauth::Error::Rejected {
                status: 429,
                detail: "rate limited".to_string(),
            })),
            Error::from(GatewayError::Internal {
                message: "storage drift".to_string(),
            }),
            Error::from(StorageError::Drift {
                message: "drift".to_string(),
            }),
        ];

        for error in other_faults {
            assert!(!error.is_client_fault(), "{error} is not a client fault");
        }
    }
}
