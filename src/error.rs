//! Error type shared by the management and outward API surfaces.

use crate::database::Error as DatabaseError;
use std::time::Duration;
use turso::Error as TursoError;

/// Status code distinguishing an upstream failure that produced an HTTP
/// response from one that failed at the transport level.
pub(crate) const UPSTREAM_UNAVAILABLE_STATUS: u16 = 502;

/// Lease duration after which an abandoned token refresh is considered dead.
pub(crate) const REFRESH_LEASE_STALE_AFTER: Duration = Duration::from_secs(90);

/// Upper bound on how long a request waits for a concurrent token refresh to
/// finish before failing.
pub(crate) const REFRESH_WAIT_LIMIT: Duration = Duration::from_secs(30);

/// Interval between polls while waiting for a concurrent token refresh.
pub(crate) const REFRESH_WAIT_INTERVAL: Duration = Duration::from_millis(250);

/// Converts a raw Turso engine failure (row conversion, statement execution)
/// into an internal API error.
impl From<TursoError> for ApiError {
    fn from(error: TursoError) -> Self {
        Self::internal(format!("database error: {error}"))
    }
}

/// Converts a database failure into an API error: constraint violations
/// surface as 409 conflicts, everything else as 500 internal errors.
impl From<DatabaseError> for ApiError {
    fn from(error: DatabaseError) -> Self {
        match &error {
            DatabaseError::Turso(turso::Error::Constraint(detail)) => {
                Self::conflict(format!("database constraint violated: {detail}"))
            }

            _ => Self::internal(format!("database error: {error}")),
        }
    }
}

/// An error surfaced to an API client, carrying everything needed to render
/// the management and OpenAI error bodies.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(crate) struct ApiError {
    /// HTTP status code for the response.
    pub(crate) status: u16,

    /// Coarse error category controlling the OpenAI `type` field.
    pub(crate) kind: ApiErrorKind,

    /// Human-readable error message.
    pub(crate) message: String,

    /// Machine-readable OpenAI error code; `None` renders as JSON `null`.
    pub(crate) code: Option<String>,

    /// Offending request parameter, when applicable.
    pub(crate) param: Option<String>,
}

/// Coarse error category controlling the OpenAI `type` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiErrorKind {
    /// The request itself is malformed or unsupported.
    InvalidRequest,

    /// Inbound authentication failed.
    Authentication,

    /// The upstream provider failed or could not be reached.
    Upstream,
}

impl ApiError {
    /// Builds an error from its parts.
    fn new(
        status: u16,
        kind: ApiErrorKind,
        code: Option<&str>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status,
            kind,
            message: message.into(),
            code: code.map(str::to_string),
            param: None,
        }
    }

    /// Builds a 400 invalid request error.
    pub(crate) fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(400, ApiErrorKind::InvalidRequest, None, message)
    }

    /// Builds a 400 invalid request error naming the offending parameter.
    pub(crate) fn invalid_parameter(param: &str, message: impl Into<String>) -> Self {
        let mut error = Self::invalid_request(message);

        error.param = Some(param.to_string());

        error
    }

    /// Builds a 404 error for an unknown or disabled model.
    pub(crate) fn model_not_found(model: &str) -> Self {
        let mut error = Self::new(
            404,
            ApiErrorKind::InvalidRequest,
            Some("model_not_found"),
            format!("The model `{model}` does not exist or you do not have access to it."),
        );

        error.param = Some("model".to_string());

        error
    }

    /// Builds a 404 error for an unknown resource.
    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(
            404,
            ApiErrorKind::InvalidRequest,
            Some("not_found"),
            message,
        )
    }

    /// Builds a 401 authentication error.
    pub(crate) fn authentication(message: impl Into<String>) -> Self {
        Self::new(401, ApiErrorKind::Authentication, None, message)
    }

    /// Builds a 401 error for a missing or invalid API key.
    pub(crate) fn invalid_api_key() -> Self {
        Self::new(
            401,
            ApiErrorKind::Authentication,
            Some("invalid_api_key"),
            "Incorrect API key provided.",
        )
    }

    /// Builds a 409 conflict error.
    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self::new(409, ApiErrorKind::InvalidRequest, Some("conflict"), message)
    }

    /// Builds an error mirroring a failed upstream HTTP response.
    pub(crate) fn upstream(status: u16, detail: impl Into<String>) -> Self {
        Self::new(
            status,
            ApiErrorKind::Upstream,
            Some("upstream_error"),
            format!("Upstream request failed: {}", detail.into()),
        )
    }

    /// Builds a 502 error for an upstream that failed without a usable HTTP
    /// response.
    pub(crate) fn upstream_unavailable(detail: impl Into<String>) -> Self {
        Self::new(
            UPSTREAM_UNAVAILABLE_STATUS,
            ApiErrorKind::Upstream,
            Some("upstream_error"),
            format!("Upstream request failed: {}", detail.into()),
        )
    }

    /// Builds a 500 internal error.
    pub(crate) fn internal(detail: impl Into<String>) -> Self {
        Self::new(500, ApiErrorKind::Upstream, Some("internal"), detail)
    }

    /// Renders the OpenAI JSON error structure.
    pub(crate) fn to_openai_json(&self) -> serde_json::Value {
        let kind = match self.kind {
            ApiErrorKind::InvalidRequest | ApiErrorKind::Authentication => "invalid_request_error",
            ApiErrorKind::Upstream => "api_error",
        };

        serde_json::json!({
            "error": {
                "message": self.message,
                "type": kind,
                "param": self.param,
                "code": self.code,
            }
        })
    }

    /// Renders the management JSON error structure.
    pub(crate) fn to_management_json(&self) -> serde_json::Value {
        let code = match self.status {
            401 => "unauthorized",
            400 => "invalid_request",
            404 => "not_found",
            409 => "conflict",
            502 => "upstream_error",
            _ => "internal",
        };

        serde_json::json!({
            "error": {
                "message": self.message,
                "code": code,
            }
        })
    }
}

/// Flattens a serialization error into an invalid request error message.
pub(crate) fn invalid_request_from_json(error: serde_json::Error) -> ApiError {
    ApiError::invalid_request(format!("Invalid JSON body: {error}"))
}
