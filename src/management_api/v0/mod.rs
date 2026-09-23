//! Management API version 0: its own route surface, self-contained error domain, and handling.
//!
//! A management API version owns everything that can break between versions — its routes, its
//! request and response payloads, and its error taxonomy — and nothing else: the management API
//! module mounts this router at [`PREFIX`] and applies the cross-version CORS policy, while
//! authentication, the body limit, and the errno-shaped 404 fallback belong to the version itself.
//!
//! Version 0's wire surface follows the seven design rules README §7.0 states: one action per
//! route, two-state fields (never tri-state), full-state updates, type-discriminated unions,
//! `<entity>_id` request references, POST-with-a-JSON-body everywhere, and responses that carry
//! only facts the caller cannot know.
//!
//! Handlers follow axum's own shape: a request body extracts through `Json` wrapped in
//! `WithRejection` with `Error` as the rejection type, and every handler returns
//! `Result<Json<..>, Error>`, so a failure leaves through the one
//! [`IntoResponse`](axum::response::IntoResponse) boundary that
//! logs and renders it — no handler matches on errors or logs them itself.

use self::error::Error;
#[cfg(feature = "management-api-typescript-bindings")]
use crate::management_api::route as route_helper;
#[cfg(feature = "management-api-typescript-bindings")]
use crate::management_api::route::{PayloadSource, RouteBinding};
use crate::{context::Context, management_api::Route as _, provider::Provider, security};
use axum::{
    Router,
    extract::{DefaultBodyLimit, OriginalUri, Request},
    http::{Method, header},
    middleware::{self, Next},
    response::{IntoResponse as _, Response},
    routing::{self, MethodFilter},
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error as DeError, Visitor},
};
use std::{
    fmt::{Error as FmtError, Formatter},
    marker::PhantomData,
};

mod api_keys;
mod error;
mod models;
mod oauth_flow;
mod providers;
mod usage;

// This version's public wire surface: every route marker and every route's request and response
// payload types. Nested wire shapes — provider and model rows, captured credentials — stay private,
// and the route contract itself is shared at [`crate::management_api::Route`].
pub use self::{
    api_keys::{
        ApiKeyEnvelope, ApiKeyResponse, CreateApiKey, CreateApiKeyRequest, DeleteApiKey,
        DeleteApiKeyRequest, ListApiKeys, ListApiKeysRequest, ListApiKeysResponse, UpdateApiKey,
        UpdateApiKeyRequest,
    },
    error::{ErrorCode, ErrorResponse},
    models::{
        CreateModel, CreateModelRequest, DeleteModel, DeleteModelRequest, DiscoverModels,
        DiscoverModelsRequest, DiscoverModelsResponse, ListModels, ListModelsRequest,
        ListModelsResponse, ModelEnvelope, ModelResponse, UpdateModel, UpdateModelRequest,
    },
    oauth_flow::{
        CompleteProviderOauth, CompleteProviderOauthRequest, ReauthorizeProvider,
        ReauthorizeProviderRequest, ReauthorizeProviderResponse, StartProviderOauth,
        StartProviderOauthRequest, StartProviderOauthResponse,
    },
    providers::{
        CreateProvider, CreateProviderRequest, DeleteProvider, DeleteProviderRequest,
        ListProviders, ListProvidersRequest, ListProvidersResponse, UpdateProvider,
        UpdateProviderRequest,
    },
    usage::{
        ApiLogResponse, ListUsage, ListUsageRequest, ListUsageResponse, QueryUsage,
        QueryUsageRequest, QueryUsageResponse, UsageBucket,
    },
};
pub use crate::{
    api_logs::{ApiLogEndpointType, ApiLogOutcome, UsageBucketKey, UsageGroupBy},
    provider::{DiscoveryState, OAuthProviderType, ProviderType},
};

/// URL prefix this version is mounted at, relative to the management API router. Its version
/// segment also names this version's TypeScript bindings file.
pub const PREFIX: &str = "/v0";

/// Every route of this version, collected link-time; iteration order is unspecified, so the
/// bindings generator sorts before rendering. This version's `route!` macro registers into it, and
/// the management API module reads it for bindings generation.
#[cfg(feature = "management-api-typescript-bindings")]
#[linkme::distributed_slice]
pub(super) static ROUTES: [RouteBinding];

/// Payload types this version's bindings publish besides its route payloads: the error body, which
/// carries the errno taxonomy this version pins.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) static EXPORTED_TYPES: &[PayloadSource] =
    &[route_helper::payload_binding::<ErrorResponse>];

/// Declares one version 0 route, registering it in `ROUTES`. The shared `route!` macro carries
/// the declaration syntax and the contract a route must satisfy; this wrapper binds the version's
/// registry, so a declaration names neither the registry nor the version.
macro_rules! route {
    ($($declaration:tt)*) => {
        $crate::management_api::route::route! {
            $crate::management_api::v0::ROUTES;
            $($declaration)*
        }
    };
}

pub(super) use route;

/// Maximum accepted request body size for the management API.
///
/// Management payloads are small JSON documents, so 2 MiB — axum's default, made explicit here so
/// the limit is deliberate and greppable — is generous; the outward client surface allows 64 MiB
/// for inline image data.
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Converts a route's method constant into axum's method filter.
///
/// # Panics
///
/// Panics when the method is not a standard HTTP method; management routes declare standard methods
/// only, so this is a route-declaration invariant.
fn method_filter(method: &Method) -> MethodFilter {
    #[expect(
        clippy::expect_used,
        reason = "management routes declare standard HTTP methods only"
    )]
    MethodFilter::try_from(method.clone()).expect("non-standard HTTP method")
}

/// Constructs this version's router with its own token authentication, 404 response, and body
/// limit. Method and path are wired from each route's [`Route`](crate::management_api::Route)
/// constants, so the `route!` declaration is the single place they are written.
///
/// The router defines no CORS policy of its own: the management API router applies the
/// cross-version one around it.
pub(super) fn router() -> Router<Context> {
    Router::new()
        .route(
            ListProviders::PATH,
            routing::on(method_filter(&ListProviders::METHOD), providers::list),
        )
        .route(
            CreateProvider::PATH,
            routing::on(method_filter(&CreateProvider::METHOD), providers::create),
        )
        .route(
            UpdateProvider::PATH,
            routing::on(method_filter(&UpdateProvider::METHOD), providers::update),
        )
        .route(
            DeleteProvider::PATH,
            routing::on(method_filter(&DeleteProvider::METHOD), providers::delete),
        )
        .route(
            StartProviderOauth::PATH,
            routing::on(
                method_filter(&StartProviderOauth::METHOD),
                oauth_flow::start,
            ),
        )
        .route(
            CompleteProviderOauth::PATH,
            routing::on(
                method_filter(&CompleteProviderOauth::METHOD),
                oauth_flow::complete,
            ),
        )
        .route(
            ReauthorizeProvider::PATH,
            routing::on(
                method_filter(&ReauthorizeProvider::METHOD),
                oauth_flow::reauthorize,
            ),
        )
        .route(
            ListApiKeys::PATH,
            routing::on(method_filter(&ListApiKeys::METHOD), api_keys::list),
        )
        .route(
            CreateApiKey::PATH,
            routing::on(method_filter(&CreateApiKey::METHOD), api_keys::create),
        )
        .route(
            UpdateApiKey::PATH,
            routing::on(method_filter(&UpdateApiKey::METHOD), api_keys::update),
        )
        .route(
            DeleteApiKey::PATH,
            routing::on(method_filter(&DeleteApiKey::METHOD), api_keys::delete),
        )
        .route(
            ListModels::PATH,
            routing::on(method_filter(&ListModels::METHOD), models::list),
        )
        .route(
            DiscoverModels::PATH,
            routing::on(method_filter(&DiscoverModels::METHOD), models::discover),
        )
        .route(
            CreateModel::PATH,
            routing::on(method_filter(&CreateModel::METHOD), models::create),
        )
        .route(
            UpdateModel::PATH,
            routing::on(method_filter(&UpdateModel::METHOD), models::update),
        )
        .route(
            DeleteModel::PATH,
            routing::on(method_filter(&DeleteModel::METHOD), models::delete),
        )
        .route(
            ListUsage::PATH,
            routing::on(method_filter(&ListUsage::METHOD), usage::list),
        )
        .route(
            QueryUsage::PATH,
            routing::on(method_filter(&QueryUsage::METHOD), usage::query),
        )
        .fallback(fallback)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn(authenticate))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

/// Authenticates the request against the bearer-less management token. The token is read from the
/// live configuration, so a reload rotates it immediately. A rejected request renders
/// [`Error::Unauthorized`] through the shared boundary, which logs it.
async fn authenticate(request: Request, next: Next) -> Response {
    let is_authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| {
            security::constant_time_eq(token, &crate::CONFIG.management_api.token)
        });

    if is_authorized {
        next.run(request).await
    } else {
        Error::Unauthorized.into_response()
    }
}

/// Handles a request whose method is not registered on an otherwise-matching path with the
/// version's error-shaped 405. axum still attaches the `Allow` header listing the registered
/// methods.
async fn method_not_allowed(method: Method, OriginalUri(uri): OriginalUri) -> Error {
    Error::MethodNotAllowed {
        method,
        path: uri.path().to_string(),
    }
}

/// Handles this version's unmapped paths with the version's error-shaped 404.
async fn fallback(method: Method, OriginalUri(uri): OriginalUri) -> Error {
    Error::UnmappedPath {
        method,
        path: uri.path().to_string(),
    }
}

/// Validates a management request identifier. Identifier content — its characters, case, and length
/// — is the caller's choice; the backend enforces only structural non-emptiness (README §7).
///
/// # Errors
///
/// Returns [`Error`] when the identifier is empty.
fn validate_identifier(param: &'static str, value: &str) -> Result<(), Error> {
    if value.is_empty() {
        return Err(Error::EmptyIdentifier { param });
    }

    Ok(())
}

/// Whether `value` holds only routing-safe characters: printable ASCII without whitespace
/// (`U+0021–U+007E`) — no spaces, no control characters, no non-ASCII text such as CJK (README
/// §7). The database schema pins the same predicate as `CHECK` constraints, so the application
/// layer and the storage layer reject exactly the same values.
fn is_valid_composition(value: &str) -> bool {
    value.bytes().all(|byte| byte.is_ascii_graphic())
}

/// Validates a machine-consumed identifier or secret (README §7): `upstream_id`, a present
/// `id_override`, gateway API key secrets, upstream static keys, and hand-replaced OAuth
/// credential fields. The value must be non-empty printable ASCII without whitespace — an empty
/// value is [`Error::EmptyIdentifier`], any other composition failure
/// [`Error::InvalidIdentifier`].
///
/// # Errors
///
/// Returns [`Error`] when the value is empty or carries a character outside the allowed set.
fn validate_composition(param: &'static str, value: &str) -> Result<(), Error> {
    validate_identifier(param, value)?;

    if !is_valid_composition(value) {
        return Err(Error::InvalidIdentifier { param });
    }

    Ok(())
}

/// Validates a model prefix (README §7): empty means no prefix, otherwise the same composition
/// every other routing token carries.
///
/// # Errors
///
/// Returns [`Error`] when a non-empty prefix carries a character outside the allowed set.
fn validate_model_prefix(value: &str) -> Result<(), Error> {
    if !value.is_empty() && !is_valid_composition(value) {
        return Err(Error::InvalidIdentifier {
            param: "model_prefix",
        });
    }

    Ok(())
}

/// Validates a static-key provider's base URL (README §7): a non-empty `http://` or `https://`
/// URL of printable ASCII without whitespace.
///
/// # Errors
///
/// Returns [`Error`] when the value has no `http(s)` scheme, has nothing after it, or carries a
/// character outside the allowed set.
fn validate_base_url(value: &str) -> Result<(), Error> {
    let after_scheme = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"));

    match after_scheme {
        Some(rest) if !rest.is_empty() && is_valid_composition(value) => Ok(()),
        _ => Err(Error::InvalidBaseUrl),
    }
}

/// The success body of a route whose operation produces no fact the caller cannot know (README §7.0
/// rule 7): an empty JSON object, shared by reference like every other response shape.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct EmptyResponse {}

/// A provider row in its management wire shape (README §7.1): a union discriminated by `type`, one
/// member per distinct field set, its `type` the union of the account kinds sharing that set —
/// so no response ever renders another kind's field as a `null` placeholder. Credentials are never
/// returned. `id` is the provider's public identifier — every management reference to an existing
/// provider goes through `provider_id` — while `name` is the unique, renameable label.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum ProviderResponse {
    /// ChatGPT account speaking the Codex Responses protocol.
    #[serde(rename = "codex_oauth")]
    CodexOauth {
        /// Provider row identifier.
        id: i64,

        /// Unique, renameable label.
        name: String,

        /// Prefix prepended to every public model name.
        model_prefix: String,

        /// Whether the provider serves traffic.
        is_enabled: bool,

        /// Unix timestamp in microseconds when the OAuth access token expires; `null` when the
        /// stored credentials carry no expiry — the not-fresh state of §3, entered only by
        /// a hand-replaced credential.
        expires_at: Option<i64>,
    },

    /// Google account speaking Gemini GenerateContent via Antigravity.
    #[serde(rename = "antigravity_oauth")]
    AntigravityOauth {
        /// Provider row identifier.
        id: i64,

        /// Unique, renameable label.
        name: String,

        /// Prefix prepended to every public model name.
        model_prefix: String,

        /// Whether the provider serves traffic.
        is_enabled: bool,

        /// Unix timestamp in microseconds when the OAuth access token expires; `null` when the
        /// stored credentials carry no expiry — the not-fresh state of §3, entered only by
        /// a hand-replaced credential.
        expires_at: Option<i64>,
    },

    /// Static API key speaking the OpenAI Responses protocol.
    #[serde(rename = "openai_response")]
    OpenAiResponse {
        /// Provider row identifier.
        id: i64,

        /// Unique, renameable label.
        name: String,

        /// Prefix prepended to every public model name.
        model_prefix: String,

        /// Whether the provider serves traffic.
        is_enabled: bool,

        /// Upstream base URL.
        base_url: String,
    },

    /// Static API key speaking the OpenAI Chat Completions protocol.
    #[serde(rename = "openai_chat_completion")]
    OpenAiChatCompletion {
        /// Provider row identifier.
        id: i64,

        /// Unique, renameable label.
        name: String,

        /// Prefix prepended to every public model name.
        model_prefix: String,

        /// Whether the provider serves traffic.
        is_enabled: bool,

        /// Upstream base URL.
        base_url: String,
    },

    /// Static API key speaking the bare Gemini GenerateContent protocol.
    #[serde(rename = "gemini_generate_content")]
    GeminiGenerateContent {
        /// Provider row identifier.
        id: i64,

        /// Unique, renameable label.
        name: String,

        /// Prefix prepended to every public model name.
        model_prefix: String,

        /// Whether the provider serves traffic.
        is_enabled: bool,

        /// Upstream base URL.
        base_url: String,
    },
}

impl ProviderResponse {
    /// Renders one provider row; credentials are never read.
    ///
    /// # Panics
    ///
    /// Panics when a static-key row carries no base URL: creation requires it and update restates
    /// it, so a missing one is storage drift.
    fn from_provider(provider: &Provider) -> Self {
        // `expires_at` mirrors the nullable storage column: an OAuth provider whose credentials an
        // exchange minted carries the exchange's expiry, while one whose credentials were replaced
        // by hand through `providers:update` carries none — the not-fresh state the gateway
        // refreshes on first use (§3).
        match provider.provider_type {
            ProviderType::CodexOauth => Self::CodexOauth {
                id: provider.id,
                name: provider.name.clone(),
                model_prefix: provider.model_prefix.clone(),
                is_enabled: provider.is_enabled,
                expires_at: provider.expires_at,
            },

            ProviderType::AntigravityOauth => Self::AntigravityOauth {
                id: provider.id,
                name: provider.name.clone(),
                model_prefix: provider.model_prefix.clone(),
                is_enabled: provider.is_enabled,
                expires_at: provider.expires_at,
            },

            ProviderType::OpenAiResponse => Self::OpenAiResponse {
                id: provider.id,
                name: provider.name.clone(),
                model_prefix: provider.model_prefix.clone(),
                is_enabled: provider.is_enabled,
                base_url: stored_base_url(provider),
            },

            ProviderType::OpenAiChatCompletion => Self::OpenAiChatCompletion {
                id: provider.id,
                name: provider.name.clone(),
                model_prefix: provider.model_prefix.clone(),
                is_enabled: provider.is_enabled,
                base_url: stored_base_url(provider),
            },

            ProviderType::GeminiGenerateContent => Self::GeminiGenerateContent {
                id: provider.id,
                name: provider.name.clone(),
                model_prefix: provider.model_prefix.clone(),
                is_enabled: provider.is_enabled,
                base_url: stored_base_url(provider),
            },
        }
    }
}

/// Reads the stored base URL of a static-key provider row: creation requires it and update
/// restates it, so a missing one was never writable through the management surface and fails
/// fast as storage drift.
///
/// # Panics
///
/// Panics when the row carries no base URL.
fn stored_base_url(provider: &Provider) -> String {
    #[expect(
        clippy::expect_used,
        reason = "a static-key row pins its base URL at write time"
    )]
    provider
        .base_url
        .clone()
        .expect("a static-key row carries a base URL")
}

/// The `providers:create` and `oauth:complete` success body (README §7.1); the response states the
/// server-assigned row identity and the exchange's expiry — the facts the caller cannot know.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ProviderEnvelope {
    /// The created provider row.
    pub provider: ProviderResponse,
}

/// A required, nullable wire field (README §7.0 rule 2): its value may be `null`, mirroring a
/// nullable storage column exactly, and the TypeScript bindings render the key as required (`T |
/// null`). A field of this type carries `#[ts(type = "<T's TypeScript spelling> | null")]`, because
/// the generic parameter has no TypeScript rendering of its own.
///
/// One tolerance is documented rather than fought: serde routes both a missing key and an explicit
/// `null` through the same none path, so the parser reads an omitted key as `null`. Under
/// full-state update semantics (rule 3) the two spellings share the one meaning — "no override" —
/// and the bindings keep well-typed clients honest by requiring the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nullable<T>(pub Option<T>);

impl<T> Serialize for Nullable<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.0 {
            Some(value) => value.serialize(serializer),
            None => serializer.serialize_none(),
        }
    }
}

impl<'de, T> Deserialize<'de> for Nullable<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        /// Accepts a value or `null`; see the type's documentation for the missing-key tolerance.
        struct NullableVisitor<T>(PhantomData<T>);

        impl<'de, T> Visitor<'de> for NullableVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = Nullable<T>;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> Result<(), FmtError> {
                formatter.write_str("a value or null")
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                T::deserialize(deserializer).map(|value| Nullable(Some(value)))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                Ok(Nullable(None))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                Ok(Nullable(None))
            }
        }

        deserializer.deserialize_option(NullableVisitor(PhantomData))
    }
}

/// The field adapter of an absent-keeps field (README §7.0 rule 2): the key may be omitted, but an
/// explicit `null` is rejected — absence and `null` are not two spellings of one state. Pair with
/// `#[serde(default)]` so a missing key defaults rather than errors, and `#[ts(optional)]` so the
/// bindings render `field?: T`.
pub(super) mod absent_keeps {
    use serde::{
        Deserialize, Deserializer,
        de::{Error, Visitor},
    };
    use std::{
        fmt::{Error as FmtError, Formatter},
        marker::PhantomData,
    };

    /// Deserializes a present value into `Some`, rejecting an explicit `null`.
    ///
    /// # Errors
    ///
    /// Returns the deserializer's error when the value is `null` or malformed.
    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        /// Accepts a value, never `null`.
        struct AbsentKeepsVisitor<T>(PhantomData<T>);

        impl<'de, T> Visitor<'de> for AbsentKeepsVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = Option<T>;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> Result<(), FmtError> {
                formatter.write_str("a value, not `null` — omit the field to keep the stored one")
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                T::deserialize(deserializer).map(Some)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: Error,
            {
                Err(Error::custom(
                    "`null` is not accepted here — omit the field to keep the stored value",
                ))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: Error,
            {
                Err(Error::custom(
                    "`null` is not accepted here — omit the field to keep the stored value",
                ))
            }
        }

        deserializer.deserialize_option(AbsentKeepsVisitor(PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Error as ManagementError, Nullable, ProviderEnvelope, ProviderResponse, absent_keeps,
        validate_base_url, validate_composition, validate_model_prefix,
    };
    use crate::provider::{Credentials, Provider, ProviderType};
    use serde::Deserialize;

    /// Verifies that a rendered provider carries exactly its own account kind's fields (README
    /// §7.1): the `type` discriminator, no field of another kind, and therefore no `null`
    /// placeholder anywhere. Pinned as rendered strings, so a renamed field, a leaked credential,
    /// or a cross-kind field fails here rather than at a client.
    ///
    /// # Panics
    ///
    /// Panics if a rendered provider diverges from the documented shape.
    #[test]
    fn renders_the_documented_provider_shapes() {
        let static_provider = Provider {
            id: 7,
            name: "acme".to_string(),
            model_prefix: "acme/".to_string(),
            provider_type: ProviderType::OpenAiChatCompletion,
            base_url: Some("https://upstream.example".to_string()),
            credentials: Credentials::ApiKey("sk-secret".to_string()),
            expires_at: None,
            is_enabled: true,
        };

        #[expect(clippy::expect_used, reason = "the provider body always serializes")]
        let rendered = serde_json::to_string(&ProviderEnvelope {
            provider: ProviderResponse::from_provider(&static_provider),
        })
        .expect("the provider body serializes");

        assert_eq!(
            rendered,
            concat!(
                r#"{"provider":{"type":"openai_chat_completion","id":7,"name":"acme","#,
                r#""model_prefix":"acme/","is_enabled":true,"#,
                r#""base_url":"https://upstream.example"}}"#,
            )
        );

        let oauth = Provider {
            id: 3,
            name: "codex".to_string(),
            model_prefix: "codex/".to_string(),
            provider_type: ProviderType::CodexOauth,
            base_url: None,
            credentials: Credentials::CodexOauth {
                access_token: "access".to_string(),
                refresh_token: "refresh".to_string(),
                account_id: "account".to_string(),
            },
            expires_at: Some(1_800_000_000_000_000),
            is_enabled: false,
        };

        #[expect(clippy::expect_used, reason = "the provider body always serializes")]
        let rendered = serde_json::to_string(&ProviderEnvelope {
            provider: ProviderResponse::from_provider(&oauth),
        })
        .expect("the provider body serializes");

        assert_eq!(
            rendered,
            concat!(
                r#"{"provider":{"type":"codex_oauth","id":3,"name":"codex","#,
                r#""model_prefix":"codex/","is_enabled":false,"expires_at":1800000000000000}}"#,
            )
        );
    }

    /// Verifies the strictness of the provider wire union (README §7.0 rule 4): an unknown field is
    /// rejected inside the addressed variant, an unknown discriminator is rejected naming the
    /// closed set, and each variant round-trips.
    ///
    /// # Panics
    ///
    /// Panics if the union parses or rejects outside its contract.
    #[test]
    fn round_trips_the_provider_union_strictly() {
        #[expect(
            clippy::unwrap_used,
            reason = "the documented payloads are constructed to parse"
        )]
        let parsed: ProviderResponse = serde_json::from_str(
            r#"{"id":3,"name":"codex","model_prefix":"codex/","type":"codex_oauth",
                "expires_at":1800000000000000,"is_enabled":true}"#,
        )
        .unwrap();

        assert!(matches!(parsed, ProviderResponse::CodexOauth { .. }));

        assert!(
            serde_json::from_str::<ProviderResponse>(
                r#"{"id":3,"name":"codex","model_prefix":"codex/","type":"codex_oauth",
                "expires_at":1800000000000000,"is_enabled":true,"base_url":"https://x"}"#
            )
            .is_err()
        );

        assert!(
            serde_json::from_str::<ProviderResponse>(
                r#"{"id":3,"name":"codex","model_prefix":"c/","type":"bogus"}"#
            )
            .is_err()
        );
    }

    /// Verifies the two field strictness adapters (README §7.0 rule 2) at their poles: a
    /// [`Nullable`] field accepts `null` (an omitted key reads as `null` — the documented
    /// tolerance), while an absent-keeps field may be omitted and rejects an explicit `null`.
    ///
    /// # Panics
    ///
    /// Panics if either adapter diverges from its pole.
    #[test]
    fn enforces_the_two_field_states() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Probe {
            id_override: Nullable<String>,

            #[serde(default, deserialize_with = "absent_keeps::deserialize")]
            api_key: Option<String>,
        }

        #[expect(
            clippy::unwrap_used,
            reason = "the probe payloads are constructed to parse"
        )]
        let null_override: Probe = serde_json::from_str(r#"{"id_override":null}"#).unwrap();

        assert_eq!(null_override.id_override, Nullable(None));
        assert_eq!(null_override.api_key, None);

        // An omitted required-nullable key reads as `null`: serde routes a missing key and an
        // explicit `null` through the same none path, and under full-state semantics the two
        // spellings share the one meaning (README §7.0 rule 2).
        #[expect(
            clippy::unwrap_used,
            reason = "the probe payloads are constructed to parse"
        )]
        let omitted_override: Probe = serde_json::from_str(r#"{}"#).unwrap();

        assert_eq!(omitted_override.id_override, Nullable(None));

        #[expect(
            clippy::unwrap_used,
            reason = "the probe payloads are constructed to parse"
        )]
        let valued: Probe = serde_json::from_str(r#"{"id_override":"x","api_key":"sk"}"#).unwrap();

        assert_eq!(valued.id_override, Nullable(Some("x".to_string())));
        assert_eq!(valued.api_key, Some("sk".to_string()));

        // An explicit `null` on an absent-keeps field is rejected instead of silently reading as
        // absence.
        assert!(
            serde_json::from_str::<Probe>(r#"{"id_override":null,"api_key":null}"#).is_err(),
            "an absent-keeps field never accepts null"
        );
    }

    /// Verifies the identifier composition rule (README §7): routing tokens and secrets accept
    /// printable ASCII without whitespace — including the punctuation real model IDs carry (`-`,
    /// `_`, `.`, `:`, `/`) — while spaces, control characters, and non-ASCII text such as CJK or
    /// emoji are rejected; an empty value fails as [`ManagementError::EmptyIdentifier`], any other
    /// composition failure as [`ManagementError::InvalidIdentifier`].
    ///
    /// # Panics
    ///
    /// Panics if a value parses or rejects outside the composition contract.
    #[test]
    fn enforces_the_identifier_composition() {
        for valid in [
            "gpt-4o",
            "qwen3:32b",
            "meta-llama/Llama-3.1-8B",
            "text-embedding-3-small",
            "codex/",
            "agy-",
            "sk-live-123_~",
            "!",
        ] {
            assert!(
                validate_composition("upstream_id", valid).is_ok(),
                "routing-safe value rejected: {valid}"
            );
        }

        assert!(matches!(
            validate_composition("upstream_id", ""),
            Err(ManagementError::EmptyIdentifier {
                param: "upstream_id"
            })
        ));

        for invalid in [
            "gpt 4o",
            " leading",
            "trailing ",
            "\ttab",
            "new\nline",
            "模型",
            "gpt-4o💥",
            "caf\u{7f}e",
        ] {
            assert!(
                matches!(
                    validate_composition("upstream_id", invalid),
                    Err(ManagementError::InvalidIdentifier { .. })
                ),
                "unroutable value accepted: {invalid:?}"
            );
        }
    }

    /// Verifies the model prefix composition (README §7): empty means no prefix, otherwise the
    /// same character set every routing token carries.
    ///
    /// # Panics
    ///
    /// Panics if a prefix parses or rejects outside its contract.
    #[test]
    fn enforces_the_model_prefix_composition() {
        assert!(validate_model_prefix("").is_ok());
        assert!(validate_model_prefix("codex/").is_ok());
        assert!(validate_model_prefix("agy-").is_ok());

        assert!(matches!(
            validate_model_prefix("codex /"),
            Err(ManagementError::InvalidIdentifier {
                param: "model_prefix"
            })
        ));

        assert!(matches!(
            validate_model_prefix("前缀/"),
            Err(ManagementError::InvalidIdentifier {
                param: "model_prefix"
            })
        ));
    }

    /// Verifies the base URL composition (README §7): a non-empty `http://` or `https://` URL of
    /// printable ASCII without whitespace — any other scheme, a bare scheme, or whitespace/CJK
    /// anywhere is rejected.
    ///
    /// # Panics
    ///
    /// Panics if a URL parses or rejects outside its contract.
    #[test]
    fn enforces_the_base_url_composition() {
        for valid in [
            "https://upstream.example.com/v1",
            "http://localhost:8080",
            "https://x.example/models?q=1",
        ] {
            assert!(
                validate_base_url(valid).is_ok(),
                "routable base URL rejected: {valid}"
            );
        }

        for invalid in [
            "",
            "https://",
            "http://",
            "ftp://upstream.example.com",
            "upstream.example.com/v1",
            "https://upstream.example.com/v 1",
            "https://例子.example.com",
        ] {
            assert!(
                matches!(
                    validate_base_url(invalid),
                    Err(ManagementError::InvalidBaseUrl)
                ),
                "unroutable base URL accepted: {invalid:?}"
            );
        }
    }
}
