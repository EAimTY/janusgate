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
use crate::{context::Context, management_api::Route as _, security};
use axum::{
    Router,
    extract::{DefaultBodyLimit, OriginalUri, Request},
    http::{Method, header},
    middleware::{self, Next},
    response::{IntoResponse as _, Response},
    routing::{self, MethodFilter},
};
use serde::{Deserialize, Serialize};

mod api_keys;
mod error;
mod models;
mod oauth_flow;
mod provider_definitions;
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
        ReauthorizeProviderRequest, StartProviderOauth, StartProviderOauthRequest,
        StartProviderOauthResponse,
    },
    provider_definitions::{
        CreateProviderDefinition, CreateProviderDefinitionRequest, DefinitionEnvelope,
        DefinitionResponse, ListProviderDefinitions, ListProviderDefinitionsRequest,
        ListProviderDefinitionsResponse, UpdateProviderDefinition, UpdateProviderDefinitionRequest,
    },
    providers::{
        CreateProvider, CreateProviderRequest, DeleteProvider, DeleteProviderRequest,
        ListProviders, ListProvidersRequest, ListProvidersResponse, UpdateProvider,
        UpdateProviderRequest,
    },
    usage::{
        ApiLogResponse, ListUsage, ListUsageRequest, ListUsageResponse, QueryUsage,
        QueryUsageRequest, QueryUsageResponse, UsageBucket, UsageBucketKey,
    },
};
/// Re-exported so this version's payloads declare the two-state field adapters of README §7.0
/// rule 2 through their own version module: `Nullable` marks a required-nullable field and
/// `absent_keeps` the absent-keeps `deserialize_with` function, both defined once in the
/// shared crate-root module `crate::serde_helpers`.
pub use crate::serde_helpers::Nullable;
pub(crate) use crate::serde_helpers::absent_keeps;
/// Re-exported for the one shared usage filter, defined with the storage reads it configures:
/// this version's usage payloads declare it, and its `serde` field attributes keep the
/// absent-keeps wire strictness through the shared `crate::serde_helpers` adapters.
pub(crate) use crate::storage::api_logs::UsageFilter;
pub use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome, UsageGroupBy},
    provider::{CredentialKind, DiscoveryState},
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
            ListProviderDefinitions::PATH,
            routing::on(
                method_filter(&ListProviderDefinitions::METHOD),
                provider_definitions::list,
            ),
        )
        .route(
            CreateProviderDefinition::PATH,
            routing::on(
                method_filter(&CreateProviderDefinition::METHOD),
                provider_definitions::create,
            ),
        )
        .route(
            UpdateProviderDefinition::PATH,
            routing::on(
                method_filter(&UpdateProviderDefinition::METHOD),
                provider_definitions::update,
            ),
        )
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

    if !crate::routing_text::is_routing_safe_text(value) {
        return Err(Error::InvalidIdentifier { param });
    }

    Ok(())
}

/// Validates a model prefix (README §7): empty means no prefix, otherwise the
/// routing-token composition — printable ASCII without whitespace and without `/` or `:` (the
/// Gemini URL path cannot address them).
///
/// # Errors
///
/// Returns [`Error`] when a non-empty prefix carries a character outside the allowed set.
fn validate_model_prefix(value: &str) -> Result<(), Error> {
    if !value.is_empty() {
        validate_routing_token("model_prefix", value)?;
    }

    Ok(())
}

/// Validates a routing token (README §7): `upstream_id`, a present `id_override`, and
/// `model_prefix` (empty means no prefix, validated by [`validate_model_prefix`]). The value
/// must be non-empty printable ASCII without whitespace and without `/` or `:` — the Gemini URL
/// path (`/models/{model}:generateContent`) cannot address them. An empty value is
/// [`Error::EmptyIdentifier`], any other composition failure [`Error::InvalidIdentifier`].
/// Existing stored rows carrying `/` or `:` are left as-is and are simply not addressable through
/// the Gemini surface.
///
/// # Errors
///
/// Returns [`Error`] when the value is empty or carries a character outside the allowed set.
fn validate_routing_token(param: &'static str, value: &str) -> Result<(), Error> {
    validate_identifier(param, value)?;

    if !crate::routing_text::is_routing_safe_token(value) {
        return Err(Error::InvalidIdentifier { param });
    }

    Ok(())
}

/// Validates a static-key provider's base URL against the shared base-URL contract of
/// [`crate::routing_text::is_base_url`] (README §7): routing-safe text plus the non-empty
/// `http://` or `https://` scheme — the same composition the provisioning boundary enforces on
/// upstream-reported URLs.
///
/// # Errors
///
/// Returns [`Error::InvalidBaseUrl`] when the value falls outside the shared base-URL contract.
fn validate_base_url(value: &str) -> Result<(), Error> {
    if crate::routing_text::is_base_url(value) {
        Ok(())
    } else {
        Err(Error::InvalidBaseUrl)
    }
}

/// Builds the rejection for a non-positive `provider_definition_version` — the one spelling every
/// path that resolves a definition reference shares: the provider write paths
/// (`providers:create`, `providers:update`) and the OAuth flow paths (`oauth:start`, and
/// `oauth:reauthorize` with an explicitly named target).
fn definition_version_invalid() -> Error {
    Error::InvalidDefinition(crate::provider::definition::ValidationError::new(
        "`provider_definition_version` must be a positive version number",
    ))
}

/// Builds the shared base-URL 400 template (README §3, §7.1): the effective definition's URLs
/// reference the base, and `tail` states why the calling path cannot satisfy the requirement —
/// a request-supplied base means the provider row must still carry one, a row-stored base means
/// the row carries none.
fn base_url_validation(definition: &crate::provider::ProviderDefinition, tail: &str) -> Error {
    Error::Validation {
        param: "base_url",
        message: format!(
            "`base_url` is required: provider definition `{}` version {} addresses its base \
             URL, {tail}",
            definition.name,
            definition.version.get(),
        ),
    }
}

/// Builds the 400 the provider writes and `oauth:start` render when the effective definition's
/// URLs reference the base but the request supplies no base URL: the written row must carry one.
fn base_url_required(definition: &crate::provider::ProviderDefinition) -> Error {
    base_url_validation(definition, "so the provider row must carry one")
}

/// Builds the 400 `oauth:reauthorize` renders when the effective target's URLs reference the base
/// but the provider row stores no base URL: the flow runs from the stored base, and a
/// reauthorization request cannot supply a replacement.
fn stored_base_url_required(definition: &crate::provider::ProviderDefinition) -> Error {
    base_url_validation(definition, "but the provider row carries none")
}

/// The success body of a route whose operation produces no fact the caller cannot know (README §7.0
/// rule 7): an empty JSON object, shared by reference like every other response shape.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct EmptyResponse {}

/// A provider row in its management wire shape: a union discriminated by the row's credential
/// kind — the one field set a row carries — with the exact definition pair, the derived wire
/// format, and the nullable base URL. Credentials are never returned. `id` is the provider's
/// public identifier — every management reference to an existing provider goes through
/// `provider_id` — while `name` is the unique, renameable label.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum ProviderResponse {
    /// An OAuth account: the row carries the expiry of its current access token.
    #[serde(rename = "oauth")]
    OAuth {
        /// The field set both credential kinds share.
        #[serde(flatten)]
        shared: SharedProviderFields,

        /// Unix timestamp in microseconds when the OAuth access token expires; `null` means no
        /// time-based refresh is scheduled.
        expires_at: Option<i64>,
    },

    /// A static API key account.
    #[serde(rename = "api_key")]
    ApiKey {
        /// The field set both credential kinds share.
        #[serde(flatten)]
        shared: SharedProviderFields,
    },
}

impl ProviderResponse {
    /// Renders one provider row; credentials are never read.
    #[must_use]
    pub(crate) fn from_provider(provider: &crate::provider::Provider) -> Self {
        let shared = SharedProviderFields {
            id: provider.id,
            name: provider.name.clone(),
            model_prefix: provider.model_prefix.clone(),
            is_enabled: provider.is_enabled,
            provider_definition_name: provider.definition.name.clone(),
            provider_definition_version: provider.definition.version.get(),
            format: provider.definition.payload.format,
            base_url: provider.base_url.clone(),
        };

        match provider.credentials {
            crate::provider::StoredCredentials::OAuth { expires_at, .. } => {
                Self::OAuth { shared, expires_at }
            }

            crate::provider::StoredCredentials::ApiKey { .. } => Self::ApiKey { shared },
        }
    }
}

/// The field set both credential kinds share, flattened into each union member.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct SharedProviderFields {
    /// Provider row identifier.
    id: i64,

    /// Unique, renameable label.
    name: String,

    /// Prefix prepended to every public model name.
    model_prefix: String,

    /// Whether the provider serves traffic.
    is_enabled: bool,

    /// The definition name the provider binds to.
    provider_definition_name: String,

    /// The exact definition version the row's requests are interpreted against.
    provider_definition_version: i64,

    /// The derived upstream wire format.
    format: crate::format::UpstreamFormat,

    /// The nullable instance base URL.
    base_url: Option<String>,
}

/// The `providers:create` and `oauth:complete` success body (README §7.1); the response states the
/// server-assigned row identity and the exchange's expiry — the facts the caller cannot know.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ProviderEnvelope {
    /// The created provider row.
    pub provider: ProviderResponse,
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "the fixtures are constructed so each unwrap lands on its documented side"
    )]

    use super::{
        Error as ManagementError, Nullable, ProviderEnvelope, ProviderResponse, absent_keeps,
        validate_base_url, validate_composition, validate_model_prefix, validate_routing_token,
    };
    use crate::{
        format::UpstreamFormat,
        provider::{Provider, StoredCredentials},
    };
    use serde::Deserialize;
    use std::{collections::BTreeMap, num::NonZeroI64};

    /// Builds one stored provider row of the given credential kind.
    ///
    /// # Panics
    ///
    /// Panics if the fixture payload does not parse, which cannot occur.
    fn provider(credentials: StoredCredentials, base_url: Option<String>) -> Provider {
        Provider {
            id: 7,
            name: "acme".to_string(),
            model_prefix: "acme-".to_string(),
            base_url,
            is_enabled: true,
            credentials,
            definition: crate::provider::ProviderDefinition {
                name: "acme-def".to_string(),
                #[expect(clippy::unwrap_used, reason = "a positive literal version")]
                version: NonZeroI64::new(3).unwrap(),
                payload: crate::provider::DefinitionPayload {
                    format: UpstreamFormat::OpenAiChatCompletion,
                    inference: serde_json::from_str(
                        r#"{"url":[{"type":"base"}],"upstream_streaming":"match_client"}"#,
                    )
                    .unwrap(),
                    credential: serde_json::from_str(r#"{"type":"api_key"}"#).unwrap(),
                    provisioning: None,
                    discovery: None,
                },
                created_at: 1,
            },
        }
    }

    /// Verifies that a rendered provider carries the definition pair, the derived format, and
    /// only its own credential kind's fields — the OAuth member carries the expiry, the
    /// static-key member does not, and neither leaks credentials.
    ///
    /// # Panics
    ///
    /// Panics if a rendered provider diverges from the documented shape.
    #[test]
    fn renders_the_documented_provider_shapes() {
        let static_provider = provider(
            StoredCredentials::ApiKey {
                api_key: "sk-secret".to_string(),
            },
            Some("https://upstream.example".to_string()),
        );

        #[expect(clippy::expect_used, reason = "the provider body always serializes")]
        let rendered = serde_json::to_string(&ProviderEnvelope {
            provider: ProviderResponse::from_provider(&static_provider),
        })
        .expect("the provider body serializes");

        assert!(rendered.contains(r#""type":"api_key""#), "{rendered}");
        assert!(
            rendered.contains(r#""provider_definition_name":"acme-def""#),
            "{rendered}"
        );
        assert!(
            rendered.contains(r#""provider_definition_version":3"#),
            "{rendered}"
        );
        assert!(
            rendered.contains(r#""format":"openai_chat_completion""#),
            "{rendered}"
        );
        assert!(
            rendered.contains(r#""base_url":"https://upstream.example""#),
            "{rendered}"
        );
        assert!(!rendered.contains(r#""api_key":"sk""#), "{rendered}");
        assert!(!rendered.contains("expires_at"), "{rendered}");

        let mut slots = BTreeMap::new();
        slots.insert("account".to_string(), "a".to_string());

        let oauth_provider = provider(
            StoredCredentials::OAuth {
                access_token: "access".to_string(),
                refresh_token: Some("refresh".to_string()),
                slots,
                expires_at: Some(1_800_000_000_000_000),
                refresh_started_at: None,
            },
            None,
        );

        #[expect(clippy::expect_used, reason = "the provider body always serializes")]
        let rendered = serde_json::to_string(&ProviderEnvelope {
            provider: ProviderResponse::from_provider(&oauth_provider),
        })
        .expect("the provider body serializes");

        assert!(rendered.contains(r#""type":"oauth""#), "{rendered}");
        assert!(
            rendered.contains(r#""expires_at":1800000000000000"#),
            "{rendered}"
        );
        assert!(rendered.contains(r#""base_url":null"#), "{rendered}");
        assert!(!rendered.contains("access_token"), "{rendered}");
    }

    /// Verifies the shared-field flattening: both credential kinds parse back with the same
    /// shared fields, and an unknown discriminator is rejected.
    ///
    /// # Panics
    ///
    /// Panics if the union parses or rejects outside its contract.
    #[test]
    fn round_trips_the_provider_union() {
        let payload = r#"{"type":"api_key","id":7,"name":"acme","model_prefix":"acme-",
            "is_enabled":true,"provider_definition_name":"d","provider_definition_version":1,
            "format":"openai_chat_completion","base_url":null}"#;

        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let parsed: ProviderResponse = serde_json::from_str(payload).unwrap();

        assert!(matches!(parsed, ProviderResponse::ApiKey { .. }));

        let payload = payload
            .replace("api_key", "oauth")
            .replace(r#""base_url":null"#, r#""base_url":null,"expires_at":null"#);

        #[expect(
            clippy::unwrap_used,
            reason = "the documented payload is constructed to parse"
        )]
        let parsed: ProviderResponse = serde_json::from_str(&payload).unwrap();

        assert!(matches!(parsed, ProviderResponse::OAuth { .. }));

        assert!(
            serde_json::from_str::<ProviderResponse>(
                r#"{"type":"bogus","id":1,"name":"x","model_prefix":"",
                    "is_enabled":true,"provider_definition_name":"d",
                    "provider_definition_version":1,"format":"openai_chat_completion"}"#
            )
            .is_err()
        );
    }

    /// Verifies the two field strictness adapters (rule 2) at their poles: a
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

        assert!(
            serde_json::from_str::<Probe>(r#"{"id_override":null,"api_key":null}"#).is_err(),
            "an absent-keeps field never accepts null"
        );
    }

    /// Verifies the identifier composition rule: routing tokens and secrets accept printable
    /// ASCII without whitespace, while spaces, control characters, and non-ASCII text are
    /// rejected; an empty value fails as [`ManagementError::EmptyIdentifier`], any other
    /// composition failure as [`ManagementError::InvalidIdentifier`].
    ///
    /// # Panics
    ///
    /// Panics if a value parses or rejects outside the composition contract.
    #[test]
    fn enforces_the_identifier_composition() {
        for valid in [
            "gpt-4o",
            "meta-llama_Llama-3.1-8B",
            "text-embedding-3-small",
            "codex-",
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
            "	tab",
            "new
line",
            "模型",
            "gpt-4o💥",
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

    /// Verifies the model prefix composition: empty means no prefix, otherwise the routing-token
    /// composition — printable ASCII without whitespace and without `/` or `:` (the Gemini URL
    /// path cannot address them).
    ///
    /// # Panics
    ///
    /// Panics if a prefix parses or rejects outside its contract.
    #[test]
    fn enforces_the_model_prefix_composition() {
        assert!(validate_model_prefix("").is_ok());
        assert!(validate_model_prefix("codex-").is_ok());
        assert!(validate_model_prefix("agy-").is_ok());

        for invalid in ["codex /", "前缀/", "codex/", "agy/", "a:b"] {
            assert!(
                matches!(
                    validate_model_prefix(invalid),
                    Err(ManagementError::InvalidIdentifier {
                        param: "model_prefix"
                    })
                ),
                "prefix accepted: {invalid:?}"
            );
        }
    }

    /// Verifies the routing-token composition: `upstream_id` and a present `id_override` reject
    /// `/` and `:` beside the generic composition.
    ///
    /// # Panics
    ///
    /// Panics if a token parses or rejects outside its contract.
    #[test]
    fn enforces_the_routing_token_composition() {
        assert!(validate_routing_token("upstream_id", "gpt-4o").is_ok());
        assert!(validate_routing_token("id_override", "custom-model_1.2").is_ok());

        for invalid in ["codex/", "a:b", "a/b", "has space", "模型", ""] {
            assert!(
                validate_routing_token("upstream_id", invalid).is_err(),
                "routing token accepted: {invalid:?}"
            );
        }
    }

    /// Verifies the base URL composition: a non-empty `http://` or `https://` URL of printable
    /// ASCII without whitespace.
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
            "ftp://upstream.example.com",
            "upstream.example.com/v1",
            "https://upstream.example.com/v 1",
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
