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
    provider::{Error as ProviderError, definition::ValidationError},
    storage::{
        Error as StorageError, api_keys::api_key_not_found_message,
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
use log::{Level, debug, error, warn};
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

    /// The exact provider definition declares no quota query.
    #[error("provider {provider_id} does not declare quota viewing")]
    QuotaNotConfigured {
        /// Addressed provider identity.
        provider_id: i64,
    },

    /// A declared quota mapping failed atomically, retaining no foreign values.
    #[error(transparent)]
    QuotaExtraction(#[from] crate::provider::quota::extraction::ExtractionError),

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

    /// A capability-override level value is not a member of the shared reasoning vocabulary
    /// (README §6.4): the closed set is the declaration's whole value domain, so a foreign string
    /// cannot become an override (nor, by the same rule, a discovery-extracted declaration).
    #[error("`{param}` carries the unknown reasoning effort `{value}`")]
    UnknownReasoningLevel {
        /// Name of the rejected field.
        param: &'static str,

        /// The rejected value.
        value: String,
    },

    /// A capability-override token cap is not a positive integer.
    #[error("`{param}` must be a positive integer")]
    NonPositiveTokenCap {
        /// Name of the rejected field.
        param: &'static str,
    },

    /// A compaction-facts override key or value is not a member of its closed vocabulary: the
    /// mechanism spellings and the `supported`/`unsupported`/`unknown` support values are the
    /// map's whole domain, so a foreign string cannot become an override.
    #[error("`{param}` carries the unknown compaction mechanism or support spelling `{value}`")]
    UnknownCompactionSupport {
        /// Name of the rejected field.
        param: &'static str,

        /// The rejected value.
        value: String,
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
    /// before any row is written instead of storing an unroutable ID (README §7.1). A routable
    /// identifier is a routing token — printable ASCII without whitespace, `/`, or `:` — and the
    /// identifier is routing data, never request content, so naming it is safe.
    #[error(
        "Upstream reported model `{upstream_id}` the gateway cannot route: identifiers must \
         contain only printable ASCII without whitespace, `/`, or `:`"
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

    /// The provider control engine failed: the device authorization, the token exchange and its
    /// refreshes, a flow stage, or the upstream model discovery — all report through the engine's
    /// one shared failure vocabulary ([`ProviderError`]), so all of them travel through this
    /// variant.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

/// The flat management error body every failed management request renders (README §7). It is part
/// of this version's published wire surface, so external frontends can parse a rendered error and
/// the bindings can publish it.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ErrorResponse {
    /// Numeric `errno` discriminant of the `ErrorCode` category.
    errno: ErrorCode,

    /// Human-readable error message.
    message: String,
}

/// Whether a body-extraction rejection is the version's body limit firing: the length-limit
/// rejection is the one body rejection with its own category — the 413 — and its own fixed wire
/// message, so the errno classification and the body rendering must agree on exactly this shape.
fn is_length_limit(rejection: &JsonRejection) -> bool {
    matches!(
        rejection,
        JsonRejection::BytesRejection(BytesRejection::FailedToBufferBody(
            FailedToBufferBody::LengthLimitError(_)
        ))
    )
}

impl Error {
    /// The errno category the failure renders as: the one classification, so the log level, the
    /// response status, and the body's `errno` can never disagree. A cause's own category decides —
    /// a vanished provider is a 404, a rejected grant a 502, an engine failure a 500 — and a body's
    /// rejection is classified by its own variant: the length-limit rejection is the version's 413,
    /// every other one an unusable request.
    fn as_errno(&self) -> ErrorCode {
        match self {
            Self::Json(rejection) if is_length_limit(rejection) => ErrorCode::PayloadTooLarge,

            Self::Json(rejection) => match rejection {
                JsonRejection::MissingJsonContentType(_) => ErrorCode::UnsupportedMediaType,
                _ => ErrorCode::InvalidRequest,
            },

            Self::Unauthorized => ErrorCode::Unauthorized,

            Self::UnmappedPath { .. }
            | Self::ProviderNotFound { .. }
            | Self::ApiKeyNotFound { .. }
            | Self::UnknownOauthState
            | Self::OauthGrantMismatch { .. }
            | Self::ProviderVanishedDuringReauthorization { .. }
            | Self::DefinitionNotFound { .. } => ErrorCode::NotFound,

            Self::MethodNotAllowed { .. } => ErrorCode::MethodNotAllowed,

            Self::QuotaNotConfigured { .. }
            | Self::EmptyIdentifier { .. }
            | Self::InvalidIdentifier { .. }
            | Self::UnknownReasoningLevel { .. }
            | Self::NonPositiveTokenCap { .. }
            | Self::UnknownCompactionSupport { .. }
            | Self::InvalidBaseUrl
            | Self::MalformedAuthorizationCode
            | Self::Validation { .. }
            | Self::InvalidDefinition(_)
            | Self::CredentialKindMismatch { .. } => ErrorCode::InvalidRequest,

            Self::Internal { .. } => ErrorCode::Internal,

            Self::StaleDefinitionVersion { .. }
            | Self::DefinitionNameTaken { .. }
            | Self::ProviderNameTaken { .. } => ErrorCode::Conflict,

            Self::QuotaExtraction(_)
            | Self::UpstreamReportedNoModels
            | Self::UpstreamReportedInvalidModelId { .. } => ErrorCode::UpstreamError,

            Self::Provider(error) => match error {
                ProviderError::NotFound { .. } => ErrorCode::NotFound,

                ProviderError::Rejected { .. }
                | ProviderError::Unavailable { .. }
                | ProviderError::Timeout { .. } => ErrorCode::UpstreamError,

                ProviderError::Internal { .. } => ErrorCode::Internal,
            },
            Self::Storage(error) => match error {
                StorageError::NotFound { .. } | StorageError::MissingAccessReference { .. } => {
                    ErrorCode::NotFound
                }
                StorageError::DuplicateAccessReference { .. } => ErrorCode::InvalidRequest,

                StorageError::Conflict { .. } | StorageError::Constraint { .. } => {
                    ErrorCode::Conflict
                }

                StorageError::InactiveApiKey { .. }
                | StorageError::Busy { .. }
                | StorageError::Drift { .. }
                | StorageError::Database(_) => ErrorCode::Internal,
            },
        }
    }

    /// Whether the failure is the calling client's own fault: every 4xx category belongs to the
    /// caller and every 5xx category to the gateway or an upstream. README §8's severity policy
    /// records client-fault rejections at debug level — an unauthorized management request
    /// excepted, see [`Error::log_level`] — and gateway or upstream failures at error level.
    fn is_client_fault(&self) -> bool {
        self.as_errno().as_status_code().is_client_error()
    }

    /// The severity the failure's one log record carries, per README §8's severity policy: an
    /// unauthorized management request is anomalous-but-tolerated behavior — a probe of the
    /// administrative surface — and is recorded at warn level; every other client fault is
    /// per-request detail recorded at debug level, the rendered response already informing the
    /// caller; a gateway or upstream failure is recorded at error level.
    fn log_level(&self) -> Level {
        match self {
            Self::Unauthorized => Level::Warn,

            _ if self.is_client_fault() => Level::Debug,

            _ => Level::Error,
        }
    }
}

/// Renders the failure into the errno-shaped response and logs it exactly once: this is the only
/// boundary a failure crosses on its way to the caller, whether it left a handler, an extractor, or
/// the router's own edges. The log record's severity is [`Error::log_level`]'s policy reading — an
/// unauthorized management request at warn, every other client fault at debug, a gateway or
/// upstream failure at error — and the error-level record carries the whole cause the response
/// withholds.
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self.log_level() {
            Level::Warn => warn!("unauthorized management request rejected: {self}"),

            Level::Debug => debug!("management request rejected: {self}"),

            _ => error!("management request failed: {self}"),
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
            Error::Json(rejection) if is_length_limit(rejection) => {
                PAYLOAD_TOO_LARGE_MESSAGE.to_string()
            }

            Error::Json(JsonRejection::BytesRejection(_)) => UNREADABLE_BODY_MESSAGE.to_string(),

            Error::Storage(StorageError::Constraint { .. }) => CONSTRAINT_MESSAGE.to_string(),

            Error::Internal { .. }
            | Error::Storage(
                StorageError::InactiveApiKey { .. }
                | StorageError::Busy { .. }
                | StorageError::Drift { .. }
                | StorageError::Database(_),
            )
            | Error::Provider(ProviderError::Internal { .. }) => INTERNAL_MESSAGE.to_string(),

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

/// Integrates safe quota failures into version zero's single logging/error boundary.
impl From<crate::provider::quota::Error> for Error {
    fn from(error: crate::provider::quota::Error) -> Self {
        match error {
            crate::provider::quota::Error::NotConfigured { provider_id } => {
                Self::QuotaNotConfigured { provider_id }
            }
            crate::provider::quota::Error::Provider(error)
            | crate::provider::quota::Error::Renewal(error) => Self::Provider(error),
            crate::provider::quota::Error::MalformedResponse => {
                Self::Provider(crate::provider::Error::Unavailable {
                    detail: "quota response could not be read as JSON".into(),
                })
            }
            crate::provider::quota::Error::Extraction(error) => Self::QuotaExtraction(error),
        }
    }
}
