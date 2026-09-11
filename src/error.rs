//! Error type shared by the management and outward API surfaces.

use crate::database::Error as DatabaseError;
use axum::{
    body::Body,
    http::{StatusCode, header},
    response::Response,
};
use log::{debug, error};
use turso::Error as TursoError;

/// Status code distinguishing an upstream failure that produced an HTTP
/// response from one that failed at the transport level.
pub(crate) const UPSTREAM_UNAVAILABLE_STATUS: u16 = 502;

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

    /// JanusGate itself failed; never a client's or upstream's fault.
    Internal,
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
    ///
    /// The upstream status is preserved verbatim for the outward gateway
    /// surface (README §6.7); management-served callers must use
    /// [`ApiError::upstream_rejected`] instead, because the management
    /// contract never forwards foreign statuses.
    pub(crate) fn upstream(status: u16, detail: impl Into<String>) -> Self {
        Self::new(
            status,
            ApiErrorKind::Upstream,
            Some("upstream_error"),
            format!("Upstream request failed: {}", detail.into()),
        )
    }

    /// Builds a 502 error for a management-served upstream call that returned
    /// a non-2xx response.
    ///
    /// The upstream status rides in the message because the management error
    /// contract (README §7) keys its `code` taxonomy on its own statuses and
    /// reserves `upstream_error` for 502; forwarding the upstream status
    /// verbatim would mislabel it as a management category.
    pub(crate) fn upstream_rejected(status: u16, detail: impl Into<String>) -> Self {
        Self::new(
            UPSTREAM_UNAVAILABLE_STATUS,
            ApiErrorKind::Upstream,
            Some("upstream_error"),
            format!(
                "Upstream request failed with status {status}: {}",
                detail.into()
            ),
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
        Self::new(500, ApiErrorKind::Internal, Some("internal"), detail)
    }

    /// Whether the error is the calling client's own fault — a malformed or
    /// unsupported request or a failed authentication — as opposed to a
    /// gateway or upstream failure.
    pub(crate) fn is_client_fault(&self) -> bool {
        matches!(
            self.kind,
            ApiErrorKind::InvalidRequest | ApiErrorKind::Authentication
        )
    }

    /// Renders the OpenAI JSON error structure.
    pub(crate) fn to_openai_json(&self) -> serde_json::Value {
        let kind = match self.kind {
            ApiErrorKind::InvalidRequest | ApiErrorKind::Authentication => "invalid_request_error",
            ApiErrorKind::Upstream | ApiErrorKind::Internal => "api_error",
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
    ///
    /// The `code` follows the error taxonomy ([`ApiErrorKind`]) rather than
    /// the raw status, so an upstream failure can never borrow a management
    /// category such as `unauthorized` or `not_found` by carrying a foreign
    /// status.
    pub(crate) fn to_management_json(&self) -> serde_json::Value {
        let code = match self.kind {
            ApiErrorKind::Authentication => "unauthorized",
            ApiErrorKind::Upstream => "upstream_error",
            ApiErrorKind::Internal => "internal",
            ApiErrorKind::InvalidRequest => match self.status {
                400 => "invalid_request",
                404 => "not_found",
                409 => "conflict",

                // SAFETY: every InvalidRequest constructor pins status 400
                // (`invalid_request` / `invalid_parameter`), 404
                // (`not_found` / `model_not_found`), or 409 (`conflict`), so
                // no other status can reach this arm; reaching it would mean
                // a constructor bug that must fail fast instead of silently
                // borrowing the `internal` management category.
                _ => unreachable!(
                    "InvalidRequest errors carry status 400, 404, or 409, never {}",
                    self.status
                ),
            },
        };

        serde_json::json!({
            "error": {
                "message": self.message,
                "code": code,
            }
        })
    }
}

/// Converts a raw Turso engine failure (row conversion, statement execution)
/// into an internal API error.
impl From<TursoError> for ApiError {
    fn from(error: TursoError) -> Self {
        Self::internal(format!("database error: {error}"))
    }
}

/// Whether the database failure is the engine's stale-snapshot busy error: a
/// read-then-write transaction attempted its first write after a concurrent
/// commit invalidated its read snapshot, and the engine refused the upgrade
/// ("database snapshot is stale, rollback and retry"; busy timeouts never
/// help). The transaction must be rolled back and the attempt retried, and
/// the retry re-reads the newer state — so a read-then-write lease such as
/// the token-refresh acquire classifies it as "not acquired" rather than as
/// failed work.
pub(crate) fn is_stale_snapshot(error: &DatabaseError) -> bool {
    matches!(error, DatabaseError::Turso(TursoError::BusySnapshot(_)))
}

/// Converts a database failure into an API error: constraint violations
/// surface as 409 conflicts, busy contention renders with a retry-flavored
/// message, everything else as 500 internal errors.
impl From<DatabaseError> for ApiError {
    fn from(error: DatabaseError) -> Self {
        match &error {
            DatabaseError::Turso(TursoError::Constraint(detail)) => {
                Self::conflict(format!("database constraint violated: {detail}"))
            }

            // Database contention is a gateway-side transient. The error
            // taxonomy (README §7) has no 503-style category and reserves
            // 502 `upstream_error` for upstream failures, so busy conditions
            // keep the 500 `internal` code with a message that names the
            // contention as retryable instead of the generic drift wording.
            DatabaseError::Turso(TursoError::Busy(detail) | TursoError::BusySnapshot(detail)) => {
                Self::internal(format!("database is busy: {detail}"))
            }

            _ => Self::internal(format!("database error: {error}")),
        }
    }
}

/// Builds a JSON response with the given status code.
///
/// Shared by every router that renders [`ApiError`] bodies (the management
/// API, the outward API, and the format sub-routers), so the status fallback
/// and static response parts have exactly one definition.
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

/// Logs one [`ApiError`] exactly once, at the response boundary about to
/// render it to its caller: a client fault is recorded at debug level (the
/// rendered response already informs the caller), while a gateway or
/// upstream failure is recorded at error level. `operation` names the
/// request the error belongs to, with whatever identifying context the
/// boundary holds (such as the model or provider name).
pub(crate) fn log_api_error(operation: &str, error: &ApiError) {
    if error.is_client_fault() {
        debug!("{operation} rejected: {error}");
    } else {
        error!("{operation} failed: {error}");
    }
}

/// Flattens a serialization error into an invalid request error message.
pub(crate) fn invalid_request_from_json(error: serde_json::Error) -> ApiError {
    ApiError::invalid_request(format!("Invalid JSON body: {error}"))
}
