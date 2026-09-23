//! Outward client API error domain: the one format-neutral taxonomy every `/api` surface shares
//! (README §6.7).
//!
//! [`Error`] is the client API's own domain — the `crate::api` module's own error type under the
//! repo's module-local `Error` convention, path-qualified against [`gateway::Error`] and the
//! management API's own `Error` — and has no coupling to the management API's error domain, which
//! stays as is. One variant per contract row, each pinning exactly one HTTP status; each wire
//! format owns one render module (see `crate::format::error_openai`, `error_gemini`,
//! `error_anthropic`) with a closed `render(&Error)` match, so compiler exhaustiveness forces every
//! format to render a new variant and a new format is one new render module.
//!
//! Domain errors convert in exactly one place: the [`From`] impls below (from [`gateway::Error`]
//! with its inner conversions for parse, storage, and OAuth failures) live here; surfaces stop
//! owning domain-to-contract classification. The request-accounting classification (`outcome` +
//! status) derives from the same variant through [`Error::accounting`], collapsing the parallel
//! mapping tables — HTTP response, stream frame, accounting — into one.
//!
//! [`gateway::Error`]: crate::gateway::Error

use crate::{
    gateway::Error as GatewayError,
    ir::InvalidRequest,
    provider::oauth::Error as OAuthError,
    storage::{Error as StorageError, api_logs::ApiLogOutcome},
};
use axum::http::StatusCode;
use log::{debug, error};

/// Fixed message every internal failure renders: the internal cause reaches the log record only —
/// the previous behavior of passing internal messages through to the client is deliberately
/// dropped (README §6.7).
const INTERNAL_MESSAGE: &str = "Internal server error.";

/// Fixed client message for authentication failures.
const INVALID_API_KEY_MESSAGE: &str = "Incorrect API key provided.";

/// Fixed client message for unregistered methods.
const METHOD_NOT_ALLOWED_MESSAGE: &str = "Method not allowed.";

/// Fixed client message for over-limit bodies.
const PAYLOAD_TOO_LARGE_MESSAGE: &str = "Request body too large.";

/// An error surfaced to an outward API client: one variant per contract row, each pinning exactly
/// one HTTP status (README §6.7).
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The request carried a missing or invalid API key.
    #[error("{INVALID_API_KEY_MESSAGE}")]
    InvalidApiKey,

    /// The request path is not routed; the surface fallback.
    #[error("Invalid URL ({method} {path})")]
    InvalidUrl {
        /// Request method named in the message.
        method: String,

        /// Request path named in the message.
        path: String,
    },

    /// The method is not registered on the addressed path.
    #[error("{METHOD_NOT_ALLOWED_MESSAGE}")]
    MethodNotAllowed,

    /// The request body exceeds the surface body limit.
    #[error("{PAYLOAD_TOO_LARGE_MESSAGE}")]
    PayloadTooLarge,

    /// The client's request failed validation.
    #[error("{message}")]
    MalformedRequest {
        /// Offending request parameter, when the rejection names one.
        param: Option<String>,

        /// Human-readable rejection message.
        message: String,
    },

    /// The requested model is unknown or disabled.
    #[error("The model `{model}` does not exist or you do not have access to it.")]
    ModelNotFound {
        /// Public model identifier the client requested.
        model: String,
    },

    /// The upstream answered a non-2xx status, mirrored verbatim. The status is carried as a
    /// [`StatusCode`] taken verbatim from the upstream response, so the mirror needs no
    /// conversion.
    #[error("Upstream request failed: {message}")]
    Upstream {
        /// Upstream status code, mirrored by the surface.
        status: StatusCode,

        /// Truncated upstream body excerpt.
        message: String,
    },

    /// The upstream failed without a usable HTTP response: a transport failure, an oversized
    /// frame, or a malformed payload.
    #[error("Upstream request failed: {message}")]
    UpstreamUnavailable {
        /// Human-readable failure detail.
        message: String,
    },

    /// A gateway-side failure: the cause reaches the log record only and the client renders the
    /// fixed message.
    #[error("{INTERNAL_MESSAGE}")]
    Internal,
}

impl Error {
    /// The HTTP status the variant pins.
    pub(crate) fn http_status(&self) -> StatusCode {
        match self {
            Self::InvalidApiKey => StatusCode::UNAUTHORIZED,

            Self::InvalidUrl { .. } | Self::ModelNotFound { .. } => StatusCode::NOT_FOUND,

            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,

            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,

            Self::MalformedRequest { .. } => StatusCode::BAD_REQUEST,

            Self::Upstream { status, .. } => *status,

            Self::UpstreamUnavailable { .. } => StatusCode::BAD_GATEWAY,

            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Classifies the variant onto its accounting outcome and the HTTP status the client is (or
    /// would be) served — the one classification every finalization point shares: a mirrored
    /// upstream non-2xx records `upstream_error` with that very status, a transport failure
    /// records `upstream_error` with 502, and a gateway-side failure records `internal` with 500.
    /// Client faults classify by their own status: authentication, routing, validation, method,
    /// and body-limit failures are the client's own fault and record `completed` with the
    /// rendered status — they never reach dispatch, so no dispatched row exists for them; the
    /// mapping stays total for stream failures rendered under an already-declared status, where
    /// only the outcome is consumed.
    pub(crate) fn accounting(&self) -> (ApiLogOutcome, i64) {
        match self {
            Self::Upstream { status, .. } => {
                (ApiLogOutcome::UpstreamError, i64::from(status.as_u16()))
            }

            Self::UpstreamUnavailable { .. } => (
                ApiLogOutcome::UpstreamError,
                i64::from(StatusCode::BAD_GATEWAY.as_u16()),
            ),

            Self::Internal => (
                ApiLogOutcome::Internal,
                i64::from(StatusCode::INTERNAL_SERVER_ERROR.as_u16()),
            ),

            error => (
                ApiLogOutcome::Completed,
                i64::from(error.http_status().as_u16()),
            ),
        }
    }

    /// Whether the error is the calling client's own fault — a malformed or unsupported request, a
    /// failed authentication, an unknown model, a wrong method, or an oversized body — as opposed
    /// to a gateway or upstream failure.
    fn is_client_fault(&self) -> bool {
        match self {
            Self::InvalidApiKey
            | Self::InvalidUrl { .. }
            | Self::MethodNotAllowed
            | Self::PayloadTooLarge
            | Self::MalformedRequest { .. }
            | Self::ModelNotFound { .. } => true,

            Self::Upstream { .. } | Self::UpstreamUnavailable { .. } | Self::Internal => false,
        }
    }
}

/// Logs one [`Error`] exactly once, at the response boundary about to render it to its caller: a
/// client fault is recorded at debug level (the rendered response already informs the caller),
/// while a gateway or upstream failure is recorded at error level. `operation` names the request
/// the error belongs to, with whatever identifying context the boundary holds (such as the model
/// or provider name).
pub(crate) fn log_error(operation: &str, error: &Error) {
    if error.is_client_fault() {
        debug!("{operation} rejected: {error}");
    } else {
        error!("{operation} failed: {error}");
    }
}

impl From<InvalidRequest> for Error {
    fn from(error: InvalidRequest) -> Self {
        Self::MalformedRequest {
            param: error.param.clone(),
            message: error.to_string(),
        }
    }
}

impl From<StorageError> for Error {
    fn from(_: StorageError) -> Self {
        Self::Internal
    }
}

impl From<OAuthError> for Error {
    fn from(error: OAuthError) -> Self {
        match error {
            // A provider vanishing between routing resolution and the lease-guarded token refresh
            // is a gateway-side concurrency condition, so it renders like any other internal
            // failure.
            OAuthError::NotFound { .. } | OAuthError::Internal { .. } => Self::Internal,

            OAuthError::Rejected { status, detail } => match StatusCode::from_u16(status) {
                Ok(status) => Self::Upstream {
                    status,
                    message: detail,
                },

                Err(_) => Self::UpstreamUnavailable {
                    message: format!("Upstream request failed with status {status}: {detail}"),
                },
            },

            OAuthError::Unavailable { detail } => Self::UpstreamUnavailable { message: detail },
        }
    }
}

impl From<GatewayError> for Error {
    fn from(error: GatewayError) -> Self {
        match error {
            GatewayError::InvalidRequest(error) => {
                let message = error.to_string();

                Self::MalformedRequest {
                    param: error.param,
                    message,
                }
            }

            GatewayError::ModelNotFound { model } => Self::ModelNotFound { model },

            GatewayError::Upstream { status, detail } => Self::Upstream {
                status,
                message: detail,
            },

            GatewayError::Unavailable { detail } => Self::UpstreamUnavailable { message: detail },

            // The OAuth classification lives once, in `From<OAuthError>`.
            GatewayError::OAuth(error) => Self::from(error),

            GatewayError::Storage(_) | GatewayError::Internal { .. } => Self::Internal,
        }
    }
}

impl From<axum::extract::rejection::BytesRejection> for Error {
    fn from(rejection: axum::extract::rejection::BytesRejection) -> Self {
        use axum::extract::rejection::BytesRejection;

        match rejection {
            BytesRejection::FailedToBufferBody(body_error) => {
                use axum::extract::rejection::FailedToBufferBody;

                match body_error {
                    FailedToBufferBody::LengthLimitError(_) => Self::PayloadTooLarge,

                    _ => Self::Internal,
                }
            }

            _ => Self::MalformedRequest {
                param: None,
                message: rejection.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::PayloadError;

    /// Verifies every variant pins its contract status (README §6.7): 401 for the key, 404 for the
    /// URL and the model, 405 for the method, 413 for the body, 400 for the malformed request, the
    /// mirrored status for the upstream, 502 for the unavailable upstream, and 500 with the fixed
    /// message for the internal failure.
    ///
    /// # Panics
    ///
    /// Panics if any variant pins a different status or the internal message leaks its cause.
    #[test]
    fn pins_one_status_per_variant() {
        let cases: Vec<(Error, StatusCode)> = vec![
            (Error::InvalidApiKey, StatusCode::UNAUTHORIZED),
            (
                Error::InvalidUrl {
                    method: "GET".to_string(),
                    path: "/nope".to_string(),
                },
                StatusCode::NOT_FOUND,
            ),
            (Error::MethodNotAllowed, StatusCode::METHOD_NOT_ALLOWED),
            (Error::PayloadTooLarge, StatusCode::PAYLOAD_TOO_LARGE),
            (
                Error::MalformedRequest {
                    param: Some("model".to_string()),
                    message: "bad model".to_string(),
                },
                StatusCode::BAD_REQUEST,
            ),
            (
                Error::ModelNotFound {
                    model: "gpt-4o".to_string(),
                },
                StatusCode::NOT_FOUND,
            ),
            (
                Error::Upstream {
                    status: StatusCode::TOO_MANY_REQUESTS,
                    message: "rate limited".to_string(),
                },
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (
                Error::UpstreamUnavailable {
                    message: "connection refused".to_string(),
                },
                StatusCode::BAD_GATEWAY,
            ),
            (Error::Internal, StatusCode::INTERNAL_SERVER_ERROR),
        ];

        for (error, status) in cases {
            assert_eq!(error.http_status(), status, "status mismatch for {error}");
        }

        assert_eq!(Error::Internal.to_string(), INTERNAL_MESSAGE);
    }

    /// Verifies the domain conversion in its one place: gateway pipeline failures land on their
    /// contract variants — routing misses become the model row, mirrored statuses stay mirrored,
    /// transport failures become unavailable, and every gateway-side failure (including a vanished
    /// provider mid-refresh and storage drift) becomes the fixed-message internal that never leaks
    /// its cause.
    ///
    /// # Panics
    ///
    /// Panics if any domain failure classifies outside its contract row.
    #[test]
    fn converts_domain_failures_in_one_place() {
        let error = Error::from(GatewayError::ModelNotFound {
            model: "m".to_string(),
        });

        assert!(matches!(error, Error::ModelNotFound { .. }));
        assert_eq!(error.http_status(), StatusCode::NOT_FOUND);

        let error = Error::from(GatewayError::Upstream {
            status: StatusCode::SERVICE_UNAVAILABLE,
            detail: "overloaded".to_string(),
        });

        assert_eq!(error.http_status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(matches!(error, Error::Upstream { .. }));

        let error = Error::from(GatewayError::from(PayloadError::new("connection refused")));

        assert_eq!(error.http_status(), StatusCode::BAD_GATEWAY);
        assert!(matches!(error, Error::UpstreamUnavailable { .. }));

        for error in [
            Error::from(GatewayError::Internal {
                message: "storage drift".to_string(),
            }),
            Error::from(GatewayError::OAuth(OAuthError::NotFound {
                message: "provider vanished during token refresh".to_string(),
            })),
            Error::from(StorageError::Drift {
                message: "drift".to_string(),
            }),
        ] {
            assert_eq!(error.http_status(), StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(error.to_string(), INTERNAL_MESSAGE);
        }

        let error = Error::from(InvalidRequest::field("model", "bad model"));

        assert!(matches!(error, Error::MalformedRequest { .. }));
        assert_eq!(error.http_status(), StatusCode::BAD_REQUEST);
    }

    /// Verifies the accounting classification derives from the same variant: mirrored statuses
    /// record `upstream_error` with that status, transport failures record `upstream_error` with
    /// 502, and gateway-side failures record `internal` with 500.
    ///
    /// # Panics
    ///
    /// Panics if any variant classifies outside its documented outcome.
    #[test]
    fn classifies_accounting_from_the_same_variant() {
        let (outcome, status) = Error::Upstream {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "rate limited".to_string(),
        }
        .accounting();

        assert_eq!((outcome, status), (ApiLogOutcome::UpstreamError, 429));

        let (outcome, status) = Error::UpstreamUnavailable {
            message: "unreachable".to_string(),
        }
        .accounting();

        assert_eq!((outcome, status), (ApiLogOutcome::UpstreamError, 502));

        let (outcome, status) = Error::Internal.accounting();

        assert_eq!((outcome, status), (ApiLogOutcome::Internal, 500));
    }
}
