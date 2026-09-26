//! Management API error domain: the failure type, its wire body, the errno taxonomy, and the single
//! logging point.
//!
//! The management surface owns its error domain end to end and shares nothing with the outward
//! OpenAI error type. [`Error`] is what a handler, an extractor, the storage layer, or an upstream
//! layer produces: one concrete variant per failure, carrying the values that identify it — the
//! rejected field, the unknown value, the provider id — so a log record prints the whole cause.
//! Every failure in the version's request lifecycle converges here — a rejected field, a malformed
//! body, a body over the version's limit, a wrong body content type, an unmatched method, an
//! unauthorized request, a storage or upstream failure — so no axum-shaped rejection ever reaches a
//! client. [`IntoResponse`] is the one boundary a failure crosses on its way to the caller: it logs
//! the failure exactly once and derives [`ErrorResponse`] from it, where a client fault states the
//! variant's own wording while an internal cause renders its category's fixed message.
//! [`ErrorCode`] is the wire's closed taxonomy: its `#[repr(u16)]` discriminant is the `errno`
//! carried in every error body, and the HTTP status is derived from the variant through
//! `AsStatusCode`, so a category and its status can never drift apart.

use crate::{
    provider::{definition::ValidationError, oauth::Error as OAuthError},
    storage::{
        Error as StorageError,
        api_keys::{api_key_not_found_message, api_key_taken_message},
        provider_definition_not_found_message, provider_name_taken_message,
        provider_not_found_message,
    },
};
use as_http_status_code::{AsStatusCode, StatusCode};
use axum::{
    Json,
    extract::rejection::{BytesRejection, FailedToBufferBody, JsonRejection},
    response::{IntoResponse, Response},
};
use http::Method;
use log::{debug, error};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as DeError};

/// Message a failed write's constraint violation renders: the engine's SQL detail stays in the log
/// record.
const CONSTRAINT_MESSAGE: &str = "the request collides with existing state";

/// Message every internal failure renders: its cause stays in the log record.
const INTERNAL_MESSAGE: &str = "internal error";

/// Message an over-limit request body renders.
const PAYLOAD_TOO_LARGE_MESSAGE: &str = "request body exceeds the management body limit";

/// Message an unreadable request body renders: the transport detail stays in the log record.
const UNREADABLE_BODY_MESSAGE: &str = "request body could not be read";

/// Machine-readable management error taxonomy.
///
/// The discriminant is the `errno` rendered in the management error body; each variant pins exactly
/// one HTTP status, derived through the `AsStatusCode` implementation. The taxonomy is part of this
/// version's published wire surface, and the TypeScript bindings publish it as a numeric enum
/// carrying those same discriminants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, AsStatusCode)]
#[cfg_attr(
    feature = "management-api-typescript-bindings",
    derive(ts_rs::TS),
    ts(repr(enum))
)]
#[repr(u16)]
pub enum ErrorCode {
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

    /// The request body exceeded the version's body limit.
    #[as_status_code(StatusCode::PAYLOAD_TOO_LARGE)]
    PayloadTooLarge = 7,

    /// The request's method is not registered on the addressed path.
    #[as_status_code(StatusCode::METHOD_NOT_ALLOWED)]
    MethodNotAllowed = 8,

    /// The request body's content type is not JSON.
    #[as_status_code(StatusCode::UNSUPPORTED_MEDIA_TYPE)]
    UnsupportedMediaType = 9,
}

impl ErrorCode {
    /// Every category, in `errno` order: the closed taxonomy's members, the one place they are
    /// enumerated besides the variants themselves.
    const ALL: [Self; 9] = [
        Self::InvalidRequest,
        Self::Unauthorized,
        Self::NotFound,
        Self::Conflict,
        Self::UpstreamError,
        Self::Internal,
        Self::PayloadTooLarge,
        Self::MethodNotAllowed,
        Self::UnsupportedMediaType,
    ];

    /// Parses one `errno` discriminant back into its category, yielding `None` for a number outside
    /// the closed taxonomy.
    fn from_errno(errno: u16) -> Option<Self> {
        Self::ALL.into_iter().find(|code| u16::from(*code) == errno)
    }
}

/// Serializes a category as its numeric `errno` discriminant, matching the rendered error body.
impl Serialize for ErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u16((*self).into())
    }
}

/// Deserializes a category from its numeric `errno` discriminant; a number outside the closed
/// taxonomy is malformed rather than silently misread.
impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let errno = u16::deserialize(deserializer)?;

        Self::from_errno(errno).ok_or_else(|| {
            <D::Error as DeError>::custom(format!("unknown management errno {errno}"))
        })
    }
}

/// A management API failure.
///
/// Concrete: every variant names one failure and carries the values that identify it, so a log
/// record printing the variant prints the whole cause. A client fault's own display is the message
/// its category promises, which is what [`ErrorResponse`] derives; an internal cause's detail never
/// crosses the wire.
#[derive(Debug, thiserror::Error)]
pub(super) enum Error {
    /// The request body failed JSON extraction: it is not the JSON the route's payload type
    /// accepts, its content type is not JSON, it exceeds the version's body limit, or it could not
    /// be read. The rejection's own display carries axum's detail for the log record.
    #[error(transparent)]
    Json(#[from] JsonRejection),

    /// The management token is missing or wrong.
    #[error("Missing or invalid management token")]
    Unauthorized,

    /// The addressed path belongs to no route of this version.
    #[error("Invalid URL ({method} {path})")]
    UnmappedPath {
        /// Method of the rejected request.
        method: Method,

        /// Path of the rejected request.
        path: String,
    },

    /// The request's method is not registered on the addressed path.
    #[error("Invalid method ({method} {path})")]
    MethodNotAllowed {
        /// Method of the rejected request.
        method: Method,

        /// Path of the rejected request.
        path: String,
    },

    /// A required identifier field is empty; names are non-empty (README
    /// §7).
    #[error("`{param}` must not be empty")]
    EmptyIdentifier {
        /// Name of the empty field.
        param: &'static str,
    },

    /// A machine-consumed identifier or secret carries a character outside printable ASCII
    /// without whitespace: spaces, control characters, and non-ASCII text such as CJK are never
    /// valid in routing IDs, model prefixes, or bearer secrets (README §7).
    #[error("`{param}` must contain only printable ASCII without whitespace")]
    InvalidIdentifier {
        /// Name of the rejected field.
        param: &'static str,
    },

    /// A static-key provider's base URL is not an `http(s)` URL of printable ASCII without
    /// whitespace (README §7).
    #[error("`base_url` must be an http:// or https:// URL of printable ASCII without whitespace")]
    InvalidBaseUrl,

    /// A provider-definition payload failed its write-time validation; the message names the
    /// offending descriptor element.
    #[error(transparent)]
    InvalidDefinition(#[from] ValidationError),

    /// A provider-definition update names a base version that is not the stored latest one.
    #[error("{message}")]
    StaleDefinitionVersion {
        /// The stale-version conflict message.
        message: String,
    },

    /// A creation names a definition name that already exists.
    #[error("Provider definition `{name}` already exists")]
    DefinitionNameTaken {
        /// The taken name.
        name: String,
    },

    /// A creation references a definition version that does not exist. The display renders
    /// through [`provider_definition_not_found_message`], the single home it shares with the
    /// storage rejection the transactional rebind constructs, so both failures read identically.
    #[error("{}", provider_definition_not_found_message(name, *version))]
    DefinitionNotFound {
        /// The definition name the request referenced.
        name: String,

        /// The definition version the request referenced.
        version: i64,
    },

    /// A provider row's credential kind disagrees with its definition version's credential
    /// recipe.
    #[error("{message}")]
    CredentialKindMismatch {
        /// The disagreement message.
        message: String,
    },

    /// A gateway invariant was violated mid-request — stored configuration drift, never a
    /// client fault. The cause rides in the log record.
    #[error("{message}")]
    Internal {
        /// Human-readable failure message for the log record.
        message: String,
    },

    /// A completion names a state whose flow runs the other grant.
    #[error("The flow at this state runs a `{state_grant}` grant and cannot complete here")]
    OauthGrantMismatch {
        /// The grant kind the flow actually runs.
        state_grant: String,
    },

    /// The addressed provider row does not exist. The display renders through
    /// [`provider_not_found_message`], the single home it shares with the storage rejection the
    /// transactional re-reads construct, so both 404s read identically.
    #[error("{}", provider_not_found_message(*.id))]
    ProviderNotFound {
        /// Identifier of the missing provider row.
        id: i64,
    },

    /// A creation names a provider name another provider already holds. The display renders
    /// through [`provider_name_taken_message`], the single home it shares with the storage
    /// rejection the checked rename constructs, so both 409s read identically.
    #[error("{}", provider_name_taken_message(.name))]
    ProviderNameTaken {
        /// The taken name.
        name: String,
    },

    /// The addressed API key row does not exist. The display renders through
    /// [`api_key_not_found_message`], the single home it shares with the storage rejection the
    /// transactional rewrite constructs, so both 404s read identically.
    #[error("{}", api_key_not_found_message(*.id))]
    ApiKeyNotFound {
        /// Identifier of the missing API key row.
        id: i64,
    },

    /// A creation or rewrite names an API key secret another key already holds. The secret itself
    /// is never named: it is a bearer credential and must stay out of rendered bodies and log
    /// records. The display renders through [`api_key_taken_message`], the single home it shares
    /// with the storage rejection the checked rewrite constructs, so both 409s read identically.
    #[error("{}", api_key_taken_message())]
    ApiKeyTaken,

    /// The completion's state token is unknown, expired, or already consumed.
    #[error("OAuth state is unknown, expired, or already consumed")]
    UnknownOauthState,

    /// The pasted authorization code carries callback-URL structure or whitespace instead of one
    /// opaque code value: a paste mistake, never a code the provider issued.
    #[error(
        "`code` must be one opaque authorization code value, \
         not a callback URL or a value with whitespace"
    )]
    MalformedAuthorizationCode,

    /// The provider vanished between the reauthorization's lookup and its credentials write — a
    /// concurrent `providers:delete`.
    #[error("provider {id} vanished during OAuth reauthorization")]
    ProviderVanishedDuringReauthorization {
        /// Identifier of the vanished provider row.
        id: i64,
    },

    /// Upstream model discovery answered with no models; the curated rows are never wiped by an
    /// empty replacement (README §7.1).
    #[error(
        "Upstream request failed: upstream reported no models; \
         refusing to replace the provider's model rows"
    )]
    UpstreamReportedNoModels,

    /// Upstream model discovery reported an identifier the gateway cannot route: the refresh fails
    /// before any row is written instead of storing an unroutable ID (README §7.1). The identifier
    /// is routing data, never request content, so naming it is safe.
    #[error(
        "Upstream reported model `{upstream_id}` the gateway cannot route: identifiers must \
         contain only printable ASCII without whitespace"
    )]
    UpstreamReportedInvalidModelId {
        /// The unroutable upstream identifier.
        upstream_id: String,
    },

    /// The request parsed but failed one of the routes' own validation rules — an out-of-range
    /// page size, a repeated grouping dimension, a filter over its value cap. The message names
    /// the offending field.
    #[error("{message}")]
    Validation {
        /// Name of the offending request parameter.
        param: &'static str,

        /// Human-readable rejection message.
        message: String,
    },

    /// The database refused the read or write, or reported stored drift.
    #[error(transparent)]
    Storage(#[from] StorageError),

    /// The provider's OAuth token endpoint failed. Upstream model discovery reports through this
    /// variant too: discovery and the token stages reject on the same upstream surface and share
    /// one failure vocabulary ([`OAuthError`]).
    #[error(transparent)]
    OAuth(#[from] OAuthError),
}

/// The flat management error body every failed management request renders (README §7). It is part
/// of this version's published wire surface, so external frontends can parse a rendered error and
/// the bindings can publish it.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ErrorResponse {
    /// Numeric `errno` discriminant of the `ErrorCode` category.
    pub errno: ErrorCode,

    /// Human-readable error message.
    pub message: String,
}

impl Error {
    /// The errno category the failure renders as: the one classification, so the log level, the
    /// response status, and the body's `errno` can never disagree. A cause's own category decides —
    /// a vanished provider is a 404, a rejected grant a 502, an engine failure a 500 — and a body's
    /// rejection is classified by its own variant: the length-limit rejection is the version's 413,
    /// every other one an unusable request.
    fn as_errno(&self) -> ErrorCode {
        match self {
            Self::Json(rejection) => match rejection {
                JsonRejection::MissingJsonContentType(_) => ErrorCode::UnsupportedMediaType,
                JsonRejection::BytesRejection(BytesRejection::FailedToBufferBody(
                    FailedToBufferBody::LengthLimitError(_),
                )) => ErrorCode::PayloadTooLarge,
                _ => ErrorCode::InvalidRequest,
            },

            Self::Unauthorized => ErrorCode::Unauthorized,

            Self::UnmappedPath { .. }
            | Self::ProviderNotFound { .. }
            | Self::ApiKeyNotFound { .. }
            | Self::UnknownOauthState
            | Self::OauthGrantMismatch { .. }
            | Self::ProviderVanishedDuringReauthorization { .. } => ErrorCode::NotFound,

            Self::MethodNotAllowed { .. } => ErrorCode::MethodNotAllowed,

            Self::EmptyIdentifier { .. }
            | Self::InvalidIdentifier { .. }
            | Self::InvalidBaseUrl
            | Self::MalformedAuthorizationCode
            | Self::Validation { .. }
            | Self::InvalidDefinition(_)
            | Self::CredentialKindMismatch { .. }
            | Self::DefinitionNotFound { .. } => ErrorCode::InvalidRequest,

            Self::Internal { .. } => ErrorCode::Internal,

            Self::StaleDefinitionVersion { .. }
            | Self::DefinitionNameTaken { .. }
            | Self::ProviderNameTaken { .. }
            | Self::ApiKeyTaken => ErrorCode::Conflict,

            Self::UpstreamReportedNoModels | Self::UpstreamReportedInvalidModelId { .. } => {
                ErrorCode::UpstreamError
            }

            Self::OAuth(error) => match error {
                OAuthError::NotFound { .. } => ErrorCode::NotFound,

                OAuthError::Rejected { .. }
                | OAuthError::Unavailable { .. }
                | OAuthError::Timeout { .. } => ErrorCode::UpstreamError,

                OAuthError::Internal { .. } => ErrorCode::Internal,
            },
            Self::Storage(error) => match error {
                StorageError::NotFound { .. } => ErrorCode::NotFound,

                StorageError::Conflict { .. } | StorageError::Constraint { .. } => {
                    ErrorCode::Conflict
                }

                StorageError::Busy { .. }
                | StorageError::Drift { .. }
                | StorageError::Database(_) => ErrorCode::Internal,
            },
        }
    }

    /// Whether the failure is the calling client's own fault: every 4xx category belongs to the
    /// caller and every 5xx category to the gateway or an upstream. README §8's severity policy
    /// records client-fault rejections at debug level and gateway or upstream failures at error
    /// level.
    fn is_client_fault(&self) -> bool {
        self.as_errno().as_status_code().is_client_error()
    }
}

/// Renders the failure into the errno-shaped response and logs it exactly once: this is the only
/// boundary a failure crosses on its way to the caller, whether it left a handler, an extractor, or
/// the router's own edges. A client fault is recorded at debug level — the rendered response
/// already informs the caller — while a gateway or upstream failure is recorded at error level with
/// the whole cause the response withholds.
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        if self.is_client_fault() {
            debug!("management request rejected: {self}");
        } else {
            error!("management request failed: {self}");
        }

        let body = ErrorResponse::from(self);

        (body.errno.as_status_code(), Json(body)).into_response()
    }
}

/// Renders the wire body from a failure.
///
/// The message is derived, never stored: a client fault states its variant's own wording — the
/// concrete display is the client-facing message by construction, including an upstream failure's
/// foreign status and excerpt — while every failure whose cause is internal renders its category's
/// fixed message, keeping the cause in the log record.
impl From<Error> for ErrorResponse {
    fn from(error: Error) -> Self {
        let errno = error.as_errno();

        let message = match &error {
            Error::Json(JsonRejection::BytesRejection(BytesRejection::FailedToBufferBody(
                FailedToBufferBody::LengthLimitError(_),
            ))) => PAYLOAD_TOO_LARGE_MESSAGE.to_string(),

            Error::Json(JsonRejection::BytesRejection(_)) => UNREADABLE_BODY_MESSAGE.to_string(),

            Error::Storage(StorageError::Constraint { .. }) => CONSTRAINT_MESSAGE.to_string(),

            Error::Storage(
                StorageError::Busy { .. } | StorageError::Drift { .. } | StorageError::Database(_),
            )
            | Error::OAuth(OAuthError::Internal { .. }) => INTERNAL_MESSAGE.to_string(),

            _ => error.to_string(),
        };

        Self { errno, message }
    }
}

impl From<ErrorCode> for u16 {
    fn from(code: ErrorCode) -> Self {
        code as Self
    }
}

#[cfg(test)]
mod tests {
    use super::{CONSTRAINT_MESSAGE, Error, ErrorCode, ErrorResponse, INTERNAL_MESSAGE};
    use crate::{
        database::Error as DatabaseError, provider::oauth::Error as OAuthError,
        storage::Error as StorageError,
    };
    use as_http_status_code::{AsStatusCode as _, StatusCode};
    use axum::{
        extract::rejection::{JsonRejection, MissingJsonContentType},
        response::IntoResponse as _,
    };
    use http::Method;

    /// Verifies that every [`ErrorCode`] carries its documented errno discriminant and derives its
    /// documented HTTP status through [`AsStatusCode`], so the taxonomy table in README §7 stays
    /// truthful.
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
            (ErrorCode::PayloadTooLarge, 7, StatusCode::PAYLOAD_TOO_LARGE),
            (
                ErrorCode::MethodNotAllowed,
                8,
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            (
                ErrorCode::UnsupportedMediaType,
                9,
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
        ];

        for (code, errno, status) in expected {
            assert_eq!(code as u16, errno);
            assert_eq!(code.as_status_code(), status);
        }
    }

    /// Verifies that a category crosses the wire as its numeric `errno` discriminant and that an
    /// unknown number is rejected, so a published error body round-trips while a number outside the
    /// closed taxonomy is malformed instead of being misread as some other category (README §7).
    ///
    /// # Panics
    ///
    /// Panics if a category serializes as anything but its errno.
    #[test]
    fn round_trips_errnos_as_numbers() {
        #[expect(clippy::expect_used, reason = "the errno body always serializes")]
        let rendered = serde_json::to_string(&ErrorCode::NotFound).expect("an errno serializes");

        assert_eq!(rendered, "3");

        #[expect(clippy::expect_used, reason = "the errno body always parses")]
        let parsed: ErrorCode = serde_json::from_str("3").expect("a known errno parses");

        assert_eq!(parsed, ErrorCode::NotFound);
        assert!(serde_json::from_str::<ErrorCode>("\"NotFound\"").is_err());
        assert!(serde_json::from_str::<ErrorCode>("10").is_err());
    }

    /// Verifies that the errno taxonomy publishes to TypeScript as a numeric enum carrying the
    /// documented discriminants, so a TypeScript client can name the `code` value a rendered error
    /// carries (README §7).
    ///
    /// # Panics
    ///
    /// Panics if the published declaration stops carrying the errnos.
    #[cfg(feature = "management-api-typescript-bindings")]
    #[test]
    fn publishes_the_errno_enum_numerically() {
        let declaration = <ErrorCode as ts_rs::TS>::decl(&ts_rs::Config::new());

        assert!(declaration.starts_with("enum ErrorCode"), "{declaration}");

        assert!(
            declaration.contains(r#""InvalidRequest" = 1"#),
            "{declaration}"
        );

        assert!(
            declaration.contains(r#""UpstreamError" = 5"#),
            "{declaration}"
        );

        assert!(declaration.contains(r#""Internal" = 6"#), "{declaration}");

        assert!(
            declaration.contains(r#""PayloadTooLarge" = 7"#),
            "{declaration}"
        );

        assert!(
            declaration.contains(r#""MethodNotAllowed" = 8"#),
            "{declaration}"
        );

        assert!(
            declaration.contains(r#""UnsupportedMediaType" = 9"#),
            "{declaration}"
        );
    }

    /// Verifies that the rendered management body carries the numeric errno and the variant's own
    /// wording, pinned at the wire level: the serialized key names and nesting are the contract
    /// README §7 documents.
    ///
    /// # Panics
    ///
    /// Panics if the rendered JSON diverges from the management shape or
    /// fails to serialize.
    #[test]
    fn renders_errno_json() {
        let body = ErrorResponse::from(Error::ProviderNotFound { id: 7 });

        #[expect(clippy::expect_used, reason = "the error body always serializes")]
        let rendered = serde_json::to_string(&body).expect("the error body serializes");

        assert_eq!(
            rendered,
            r#"{"errno":3,"message":"Provider 7 does not exist"}"#
        );

        // The taken-key conflict names no secret: the value is a bearer credential and must never
        // reach a rendered body.
        let body = ErrorResponse::from(Error::ApiKeyTaken);

        #[expect(clippy::expect_used, reason = "the error body always serializes")]
        let rendered = serde_json::to_string(&body).expect("the error body serializes");

        assert_eq!(
            rendered,
            r#"{"errno":4,"message":"API key is already taken"}"#
        );
    }

    /// Verifies that the response carries the category's derived status, in particular the pinned
    /// 502 for upstream failures.
    ///
    /// # Panics
    ///
    /// Panics if the rendered response status diverges from the category.
    #[test]
    fn renders_derived_status() {
        let response = Error::ProviderNameTaken {
            name: "acme".to_string(),
        }
        .into_response();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    /// Verifies that every cause the handlers can surface is classified onto the management
    /// taxonomy: storage rows split per outcome, an upstream failure always pins 502
    /// `UpstreamError`, a vanished provider stays a 404, route-level request validation and
    /// credentials parsing land on 400, and a wrong body content type lands on the 415 category.
    ///
    /// # Panics
    ///
    /// Panics if a cause's category diverges.
    #[test]
    fn maps_domain_errors_by_category() {
        let database_busy = StorageError::Busy {
            detail: "locked".to_string(),
        };

        let cases = [
            (
                Error::from(StorageError::NotFound {
                    message: "missing".to_string(),
                }),
                ErrorCode::NotFound,
            ),
            (
                Error::from(StorageError::Conflict {
                    message: "taken".to_string(),
                }),
                ErrorCode::Conflict,
            ),
            (
                Error::from(StorageError::Constraint {
                    detail: "unique".to_string(),
                }),
                ErrorCode::Conflict,
            ),
            (Error::from(database_busy), ErrorCode::Internal),
            (
                Error::from(StorageError::Drift {
                    message: "drift".to_string(),
                }),
                ErrorCode::Internal,
            ),
            (
                Error::from(StorageError::Database(DatabaseError::PoolClosed)),
                ErrorCode::Internal,
            ),
            (
                Error::from(OAuthError::Rejected {
                    status: 429,
                    detail: "rate limited".to_string(),
                }),
                ErrorCode::UpstreamError,
            ),
            (
                Error::from(OAuthError::Unavailable {
                    detail: "unreachable".to_string(),
                }),
                ErrorCode::UpstreamError,
            ),
            (
                Error::from(OAuthError::Timeout {
                    detail: "cut off".to_string(),
                }),
                ErrorCode::UpstreamError,
            ),
            (
                Error::from(OAuthError::NotFound {
                    message: "vanished".to_string(),
                }),
                ErrorCode::NotFound,
            ),
            (
                Error::from(OAuthError::Internal {
                    message: "lease".to_string(),
                }),
                ErrorCode::Internal,
            ),
            (
                Error::Validation {
                    param: "id",
                    message: "empty".to_string(),
                },
                ErrorCode::InvalidRequest,
            ),
            (
                Error::ProviderVanishedDuringReauthorization { id: 7 },
                ErrorCode::NotFound,
            ),
            (Error::ApiKeyNotFound { id: 7 }, ErrorCode::NotFound),
            (Error::ApiKeyTaken, ErrorCode::Conflict),
            (Error::UnknownOauthState, ErrorCode::NotFound),
            (Error::MalformedAuthorizationCode, ErrorCode::InvalidRequest),
            (
                Error::InvalidIdentifier {
                    param: "upstream_id",
                },
                ErrorCode::InvalidRequest,
            ),
            (Error::InvalidBaseUrl, ErrorCode::InvalidRequest),
            (
                Error::StaleDefinitionVersion {
                    message: "stale".to_string(),
                },
                ErrorCode::Conflict,
            ),
            (
                Error::DefinitionNotFound {
                    name: "acme".to_string(),
                    version: 2,
                },
                ErrorCode::InvalidRequest,
            ),
            (
                Error::CredentialKindMismatch {
                    message: "kinds disagree".to_string(),
                },
                ErrorCode::InvalidRequest,
            ),
            (Error::UpstreamReportedNoModels, ErrorCode::UpstreamError),
            (
                Error::from(JsonRejection::MissingJsonContentType(
                    MissingJsonContentType::default(),
                )),
                ErrorCode::UnsupportedMediaType,
            ),
        ];

        for (error, code) in cases {
            assert_eq!(error.as_errno(), code, "category mismatch for {error}");
        }
    }

    /// Verifies that the client-fault classification partitions the taxonomy exactly as README §8's
    /// severity policy prescribes: client-fault rejections — malformed bodies, failed
    /// authentication, missing resources, conflicts, an over-limit body, an unmatched method, a
    /// wrong body content type — are logged at debug level, while gateway and upstream failures are
    /// logged at error level.
    ///
    /// # Panics
    ///
    /// Panics if a category's classification diverges from the policy.
    #[test]
    fn classifies_client_faults() {
        let cases = [
            (Error::EmptyIdentifier { param: "name" }, true),
            (
                Error::InvalidIdentifier {
                    param: "upstream_id",
                },
                true,
            ),
            (Error::InvalidBaseUrl, true),
            (Error::MalformedAuthorizationCode, true),
            (
                Error::CredentialKindMismatch {
                    message: "kinds disagree".to_string(),
                },
                true,
            ),
            (Error::Unauthorized, true),
            (Error::ProviderNotFound { id: 7 }, true),
            (
                Error::ProviderNameTaken {
                    name: "acme".to_string(),
                },
                true,
            ),
            (Error::ApiKeyNotFound { id: 7 }, true),
            (Error::ApiKeyTaken, true),
            (
                Error::MethodNotAllowed {
                    method: Method::PATCH,
                    path: "/providers:list".to_string(),
                },
                true,
            ),
            (
                Error::from(JsonRejection::MissingJsonContentType(
                    MissingJsonContentType::default(),
                )),
                true,
            ),
            (Error::UpstreamReportedNoModels, false),
            (
                Error::UpstreamReportedInvalidModelId {
                    upstream_id: "gpt 4o".to_string(),
                },
                false,
            ),
        ];

        for (error, is_client_fault) in cases {
            assert_eq!(
                error.is_client_fault(),
                is_client_fault,
                "classification mismatch for {error}"
            );
        }
    }

    /// Verifies that a wrong body content type renders the 415 category with the rejection's own
    /// wording — the one extraction failure axum lets a test construct directly, the body-buffering
    /// rejections carrying private engine detail.
    ///
    /// # Panics
    ///
    /// Panics if the rendered body diverges from the category and wording.
    #[test]
    fn renders_wrong_content_type() {
        let body = ErrorResponse::from(Error::from(JsonRejection::MissingJsonContentType(
            MissingJsonContentType::default(),
        )));

        assert_eq!(body.errno, ErrorCode::UnsupportedMediaType);
        assert_eq!(
            body.message,
            "Expected request with `Content-Type: application/json`"
        );
    }

    /// Verifies that an upstream failure's message is the domain failure's display verbatim, so the
    /// foreign status and excerpt the management contract promises stay owned by the domain module.
    ///
    /// # Panics
    ///
    /// Panics if the preserved message diverges.
    #[test]
    fn preserves_domain_messages() {
        let body = ErrorResponse::from(Error::from(OAuthError::Rejected {
            status: 429,
            detail: "rate limited".to_string(),
        }));

        assert_eq!(
            body.message,
            "Upstream request failed with status 429: rate limited"
        );
    }

    /// Verifies the split between the failure and the body: an internal cause keeps its detail for
    /// the log record while the client learns only the category's message, and a constraint's
    /// engine detail stays internal the same way.
    ///
    /// # Panics
    ///
    /// Panics if an internal detail reaches the wire body or the log-facing
    /// failure stops carrying it.
    #[test]
    fn withholds_internal_causes_from_the_body() {
        let busy = Error::from(StorageError::Busy {
            detail: "database is locked".to_string(),
        });

        assert!(busy.to_string().contains("database is locked"));

        let body = ErrorResponse::from(busy);

        assert_eq!(body.errno, ErrorCode::Internal);
        assert_eq!(body.message, INTERNAL_MESSAGE);

        let constraint = Error::from(StorageError::Constraint {
            detail: "UNIQUE constraint failed: providers.name".to_string(),
        });
        let body = ErrorResponse::from(constraint);

        assert_eq!(body.errno, ErrorCode::Conflict);
        assert_eq!(body.message, CONSTRAINT_MESSAGE);
    }
}
