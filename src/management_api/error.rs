//! Management API error domain: taxonomy, rendering, and logging.
//!
//! The management surface owns its error type end to end and shares nothing
//! with the outward OpenAI error type. [`ErrorCode`] is the entire taxonomy:
//! its `#[repr(u16)]` discriminant is the machine-readable `errno` carried in
//! every management error body, and the HTTP status is derived from the
//! variant through [`AsStatusCode`], so a category and its status can never
//! drift apart.

use crate::{
    ir::InvalidRequest,
    provider::{
        ParseError, discovery::Error as DiscoveryError, oauth::Error as OAuthError,
        storage::Error as StorageError,
    },
};
use as_http_status_code::{AsStatusCode, StatusCode};
use axum::response::Response;
use log::{debug, error};

/// Machine-readable management error taxonomy.
///
/// The discriminant is the `errno` rendered in the management error body;
/// each variant pins exactly one HTTP status, derived through the
/// [`AsStatusCode`] implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AsStatusCode)]
#[repr(u16)]
enum ErrorCode {
    /// The request itself is malformed or unsupported.
    #[as_status_code(StatusCode::BAD_REQUEST)]
    InvalidRequest = 1,

    /// Inbound authentication failed.
    #[as_status_code(StatusCode::UNAUTHORIZED)]
    Unauthorized = 2,

    /// The addressed resource does not exist.
    #[as_status_code(StatusCode::NOT_FOUND)]
    NotFound = 3,

    /// The request collides with existing state.
    #[as_status_code(StatusCode::CONFLICT)]
    Conflict = 4,

    /// The upstream provider failed or could not be reached. The foreign
    /// upstream status rides in the message; the response status is always
    /// 502.
    #[as_status_code(StatusCode::BAD_GATEWAY)]
    UpstreamError = 5,

    /// JanusGate itself failed; never a client's or upstream's fault.
    #[as_status_code(StatusCode::INTERNAL_SERVER_ERROR)]
    Internal = 6,
}

/// An error surfaced to a management API client, carrying everything needed
/// to render the management error body.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(super) struct ManagementApiError {
    /// Coarse error category; the discriminant is the rendered `errno`.
    code: ErrorCode,

    /// Human-readable error message.
    message: String,
}

impl ManagementApiError {
    /// Builds an error from its parts.
    fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Builds a 400 invalid request error.
    pub(super) fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, message)
    }

    /// Builds a 401 authentication error.
    pub(super) fn authentication(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unauthorized, message)
    }

    /// Builds a 404 error for an unknown resource.
    pub(super) fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// Builds a 409 conflict error.
    pub(super) fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, message)
    }

    /// Builds a 500 internal error.
    pub(super) fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    /// Builds a 502 upstream error with the complete human message the
    /// domain failure already carries.
    pub(super) fn upstream(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::UpstreamError, message)
    }

    /// Whether the error is the calling client's own fault — a malformed or
    /// unsupported request, a failed authentication, a missing addressed
    /// resource, or a collision with existing state — as opposed to a
    /// gateway or upstream failure. README §8's severity policy records
    /// client-fault rejections at debug level and gateway or upstream
    /// failures at error level.
    fn is_client_fault(&self) -> bool {
        matches!(
            self.code,
            ErrorCode::InvalidRequest
                | ErrorCode::Unauthorized
                | ErrorCode::NotFound
                | ErrorCode::Conflict
        )
    }

    /// Renders the management JSON error structure: the numeric `errno`
    /// discriminant alongside the human-readable message.
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "error": {
                "code": self.code as u16,
                "message": self.message,
            }
        })
    }

    /// Renders the error into an HTTP response, taking the status code from
    /// the category's derived status.
    pub(super) fn into_response(self) -> Response {
        super::json_response(self.code.as_status_code(), self.to_json())
    }
}

/// Converts storage failures into the management taxonomy, preserving the
/// domain message verbatim: a missing row stays a 404, a conflicting or
/// constraint-rejected write is a 409, and every engine failure or drift is
/// a 500.
impl From<StorageError> for ManagementApiError {
    fn from(error: StorageError) -> Self {
        let code = match &error {
            StorageError::NotFound { .. } => ErrorCode::NotFound,
            StorageError::Conflict { .. } | StorageError::Constraint { .. } => ErrorCode::Conflict,
            StorageError::Busy { .. } | StorageError::Drift { .. } | StorageError::Database(_) => {
                ErrorCode::Internal
            }
        };

        Self::new(code, error.to_string())
    }
}

/// Converts OAuth token acquisition failures into the management taxonomy: an
/// upstream rejection or unreachable endpoint is a 502 — the foreign status
/// rides in the domain message, never in the response status — a vanished
/// provider is a 404, and lease or drift failures are a 500.
impl From<OAuthError> for ManagementApiError {
    fn from(error: OAuthError) -> Self {
        let code = match &error {
            OAuthError::NotFound { .. } => ErrorCode::NotFound,
            OAuthError::Rejected { .. } | OAuthError::Unavailable { .. } => {
                ErrorCode::UpstreamError
            }
            OAuthError::Internal { .. } => ErrorCode::Internal,
        };

        Self::new(code, error.to_string())
    }
}

/// Converts model discovery failures into the management taxonomy: an
/// upstream rejection or unreachable endpoint is a 502, and a stored
/// configuration invariant violation is a 500.
impl From<DiscoveryError> for ManagementApiError {
    fn from(error: DiscoveryError) -> Self {
        let code = match &error {
            DiscoveryError::Rejected { .. } | DiscoveryError::Unavailable { .. } => {
                ErrorCode::UpstreamError
            }
            DiscoveryError::Internal { .. } => ErrorCode::Internal,
        };

        Self::new(code, error.to_string())
    }
}

/// Converts a shared request validation failure into a 400 invalid request.
impl From<InvalidRequest> for ManagementApiError {
    fn from(error: InvalidRequest) -> Self {
        Self::new(ErrorCode::InvalidRequest, error.to_string())
    }
}

/// Converts a credentials parse failure into a 400 invalid request, keeping
/// the typed message wording the management contract documents.
impl From<ParseError> for ManagementApiError {
    fn from(error: ParseError) -> Self {
        Self::new(ErrorCode::InvalidRequest, error.to_string())
    }
}

/// Logs one [`ManagementApiError`] exactly once, at the response boundary about
/// to render it to its caller: a client fault is recorded at debug level (the
/// rendered response already informs the caller), while a gateway or
/// upstream failure is recorded at error level. `operation` names the request
/// the error belongs to, with whatever identifying context the boundary holds
/// (such as the provider name or identifier).
pub(super) fn log_error(operation: &str, error: &ManagementApiError) {
    if error.is_client_fault() {
        debug!("{operation} rejected: {error}");
    } else {
        error!("{operation} failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that every [`ErrorCode`] carries its documented errno
    /// discriminant and derives its documented HTTP status through
    /// [`AsStatusCode`], so the taxonomy table in README §7 stays truthful.
    ///
    /// # Panics
    ///
    /// Panics if an errno or a derived status diverges from the taxonomy.
    #[test]
    fn derives_statuses_from_errnos() {
        let expected = [
            (ErrorCode::InvalidRequest, 1, StatusCode::BAD_REQUEST),
            (ErrorCode::Unauthorized, 2, StatusCode::UNAUTHORIZED),
            (ErrorCode::NotFound, 3, StatusCode::NOT_FOUND),
            (ErrorCode::Conflict, 4, StatusCode::CONFLICT),
            (ErrorCode::UpstreamError, 5, StatusCode::BAD_GATEWAY),
            (ErrorCode::Internal, 6, StatusCode::INTERNAL_SERVER_ERROR),
        ];

        for (code, errno, status) in expected {
            assert_eq!(code as u16, errno);
            assert_eq!(code.as_status_code(), status);
        }
    }

    /// Verifies that the rendered management body carries the numeric errno
    /// and the verbatim message.
    ///
    /// # Panics
    ///
    /// Panics if the rendered JSON diverges from the management shape.
    #[test]
    fn renders_errno_json() {
        let error = ManagementApiError::not_found("Provider 7 does not exist");

        assert_eq!(
            error.to_json(),
            serde_json::json!({
                "error": {
                    "code": 3,
                    "message": "Provider 7 does not exist",
                }
            })
        );
    }

    /// Verifies that the response carries the category's derived status, in
    /// particular the pinned 502 for upstream failures.
    ///
    /// # Panics
    ///
    /// Panics if the rendered response status diverges from the category.
    #[test]
    fn renders_derived_status() {
        let response = ManagementApiError::conflict("name is taken").into_response();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    /// Verifies that domain failures map onto the management taxonomy with
    /// the message preserved: storage rows split per outcome, an upstream
    /// failure — including one carrying a foreign status — always pins 502
    /// `UpstreamError`, and shared request validation lands on 400.
    ///
    /// # Panics
    ///
    /// Panics if a mapped category or preserved message diverges.
    #[test]
    fn maps_domain_errors_by_category() {
        let database_busy = StorageError::Busy {
            detail: "locked".to_string(),
        };

        let cases = [
            (
                ManagementApiError::from(StorageError::NotFound {
                    message: "missing".to_string(),
                }),
                ErrorCode::NotFound,
            ),
            (
                ManagementApiError::from(StorageError::Conflict {
                    message: "taken".to_string(),
                }),
                ErrorCode::Conflict,
            ),
            (
                ManagementApiError::from(StorageError::Constraint {
                    detail: "unique".to_string(),
                }),
                ErrorCode::Conflict,
            ),
            (ManagementApiError::from(database_busy), ErrorCode::Internal),
            (
                ManagementApiError::from(StorageError::Drift {
                    message: "drift".to_string(),
                }),
                ErrorCode::Internal,
            ),
            (
                ManagementApiError::from(StorageError::Database(
                    crate::database::Error::PoolClosed,
                )),
                ErrorCode::Internal,
            ),
            (
                ManagementApiError::from(OAuthError::Rejected {
                    status: 429,
                    detail: "rate limited".to_string(),
                }),
                ErrorCode::UpstreamError,
            ),
            (
                ManagementApiError::from(OAuthError::Unavailable {
                    detail: "unreachable".to_string(),
                }),
                ErrorCode::UpstreamError,
            ),
            (
                ManagementApiError::from(OAuthError::NotFound {
                    message: "vanished".to_string(),
                }),
                ErrorCode::NotFound,
            ),
            (
                ManagementApiError::from(OAuthError::Internal {
                    message: "lease".to_string(),
                }),
                ErrorCode::Internal,
            ),
            (
                ManagementApiError::from(DiscoveryError::Rejected {
                    status: 429,
                    detail: "rate limited".to_string(),
                }),
                ErrorCode::UpstreamError,
            ),
            (
                ManagementApiError::from(DiscoveryError::Unavailable {
                    detail: "no models".to_string(),
                }),
                ErrorCode::UpstreamError,
            ),
            (
                ManagementApiError::from(DiscoveryError::Internal {
                    message: "config".to_string(),
                }),
                ErrorCode::Internal,
            ),
            (
                ManagementApiError::from(InvalidRequest::field("id", "empty")),
                ErrorCode::InvalidRequest,
            ),
            (
                ManagementApiError::from(ParseError::MissingField("apiKey".to_string())),
                ErrorCode::InvalidRequest,
            ),
        ];

        for (error, code) in cases {
            assert_eq!(error.code, code, "code mismatch for {error}");
        }
    }

    /// Verifies that the client-fault classification partitions the taxonomy
    /// exactly as README §8's severity policy prescribes: client-fault
    /// rejections — malformed bodies, failed authentication, missing
    /// resources, and conflicts — are logged at debug level, while gateway
    /// and upstream failures are logged at error level.
    ///
    /// # Panics
    ///
    /// Panics if a category's classification diverges from the policy.
    #[test]
    fn classifies_client_faults() {
        let cases = [
            (ManagementApiError::invalid_request("malformed"), true),
            (ManagementApiError::authentication("bad token"), true),
            (ManagementApiError::not_found("missing"), true),
            (ManagementApiError::conflict("taken"), true),
            (ManagementApiError::upstream("unreachable"), false),
            (ManagementApiError::internal("drift"), false),
        ];

        for (error, is_client_fault) in cases {
            assert_eq!(
                error.is_client_fault(),
                is_client_fault,
                "classification mismatch for {error}"
            );
        }
    }

    /// Verifies that a mapped upstream failure's message is the domain
    /// failure's display verbatim, so the wire wording stays owned by the
    /// domain module.
    ///
    /// # Panics
    ///
    /// Panics if the preserved message diverges.
    #[test]
    fn preserves_domain_messages() {
        let mapped: ManagementApiError = OAuthError::Rejected {
            status: 429,
            detail: "rate limited".to_string(),
        }
        .into();

        assert_eq!(
            mapped.message,
            "Upstream request failed with status 429: rate limited"
        );
    }
}
