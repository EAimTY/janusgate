//! Management API version 0: its own route surface, self-contained error domain, and handling.
//!
//! A management API version owns everything that can break between versions — its routes, its
//! request and response payloads, and its error taxonomy — and nothing else: the management API
//! module mounts this router at [`PREFIX`] and applies the cross-version CORS policy, while
//! authentication, the body limit, and the errno-shaped 404 fallback belong to the version itself.
//!
//! Version 0's wire surface follows the seven design rules `docs/reference/management-api.md`
//! states: one action per route, two-state fields (never tri-state), full-state updates,
//! type-discriminated unions, `<entity>_id` request references, POST-with-a-JSON-body everywhere,
//! and responses that carry only facts the caller cannot know.
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
use std::{future::Future, num::NonZeroI64};

mod api_keys;
mod error;
mod model_routes;
mod models;
mod oauth_flow;
mod provider_availability;
mod provider_definitions;
mod provider_pools;
mod provider_quota;
mod providers;
mod usage;

#[cfg(test)]
mod tests;

// The route markers this version's router mounts, plus the error body the bindings export besides
// the route payloads; every other payload type stays in the file of the route declaring it, and
// the route contract itself is shared at `crate::management_api::Route`.
#[cfg(feature = "management-api-typescript-bindings")]
use self::error::ErrorResponse;
use self::{
    api_keys::{CreateApiKey, DeleteApiKey, ListApiKeys, UpdateApiKey},
    model_routes::{CreateModelRoute, DeleteModelRoute, ListModelRoutes, UpdateModelRoute},
    models::{CreateModel, DeleteModel, DiscoverModels, ListModels, UpdateModel},
    oauth_flow::{CompleteProviderOauth, ReauthorizeProvider, StartProviderOauth},
    provider_availability::{GetProviderAvailability, ResetProviderAvailability},
    provider_definitions::{
        CreateProviderDefinition, ListProviderDefinitions, UpdateProviderDefinition,
    },
    provider_pools::{
        CreateProviderPool, DeleteProviderPool, ListProviderPools, UpdateProviderPool,
    },
    provider_quota::GetProviderQuota,
    providers::{CreateProvider, DeleteProvider, ListProviders, UpdateProvider},
    usage::{ListUsage, QueryUsage},
};
/// The private import through which this version's payloads declare required-nullable fields
/// (`docs/reference/management-api.md` two-state fields), defined once in the shared
/// crate-root module `crate::serde_helpers`.
use crate::serde_helpers::Nullable;
/// The private import of the one shared usage filter this version's usage payloads declare,
/// defined with the storage reads it configures in `crate::storage::api_logs`.
use crate::storage::api_logs::UsageFilter;
use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome},
    serde_helpers::absent_keeps,
};

/// URL prefix this version is mounted at, relative to the management API router. Its version
/// segment also names this version's TypeScript bindings file.
pub(super) const PREFIX: &str = "/v0";

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

use route;

/// Mounts version 0 routes on a router under construction: invoked with the router being built
/// followed by `marker => handler` pairs in registration order, where each pair expands to
/// exactly the shared `.route(...)` wiring, taking the path and the method from the route
/// marker's [`Route`](crate::management_api::Route) constants, so a mount names the marker type
/// and the handler only. The pairs are expanded in the written order, which is the router's
/// route registration order.
macro_rules! mount {
    ($router:expr, $($marker:ident => $handler:path),+,) => {
        $router
            $(.route(
                $marker::PATH,
                routing::on(method_filter(&$marker::METHOD), $handler),
            ))+
    };
}

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
/// constants through the local [`mount!`] macro, so the `route!` declaration is the single place
/// they are written.
///
/// The router defines no CORS policy of its own: the management API router applies the
/// cross-version one around it.
pub(super) fn router() -> Router<Context> {
    mount! {
        Router::new(),
        ListProviderDefinitions => provider_definitions::list,
        CreateProviderDefinition => provider_definitions::create,
        UpdateProviderDefinition => provider_definitions::update,
        ListProviders => providers::list,
        GetProviderQuota => provider_quota::get,
        GetProviderAvailability => provider_availability::get,
        ResetProviderAvailability => provider_availability::reset,
        CreateProvider => providers::create,
        UpdateProvider => providers::update,
        DeleteProvider => providers::delete,
        StartProviderOauth => oauth_flow::start,
        CompleteProviderOauth => oauth_flow::complete,
        ReauthorizeProvider => oauth_flow::reauthorize,
        ListApiKeys => api_keys::list,
        CreateApiKey => api_keys::create,
        UpdateApiKey => api_keys::update,
        DeleteApiKey => api_keys::delete,
        ListModels => models::list,
        DiscoverModels => models::discover,
        CreateModel => models::create,
        UpdateModel => models::update,
        DeleteModel => models::delete,
        ListProviderPools => provider_pools::list,
        CreateProviderPool => provider_pools::create,
        UpdateProviderPool => provider_pools::update,
        DeleteProviderPool => provider_pools::delete,
        ListModelRoutes => model_routes::list,
        CreateModelRoute => model_routes::create,
        UpdateModelRoute => model_routes::update,
        DeleteModelRoute => model_routes::delete,
        ListUsage => usage::list,
        QueryUsage => usage::query,
    }
    .fallback(fallback)
    .method_not_allowed_fallback(method_not_allowed)
    .layer(middleware::from_fn(authenticate))
    .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

/// Authenticates the request against the bearer-less management token. The token is read from
/// the live configuration, so a reload rotates it immediately; a reload that removed the section
/// while the routes remain mounted — the mount is decided at startup — leaves no token to match,
/// so every request is rejected until the restart unmounts the routes. A rejected request
/// renders [`Error::Unauthorized`] through the shared boundary, which logs it.
async fn authenticate(request: Request, next: Next) -> Response {
    let is_authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| {
            crate::CONFIG
                .management_api
                .as_ref()
                .is_some_and(|section| security::constant_time_eq(token, &section.token))
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

/// Runs one mutation and its post-commit derived-state invalidation on a detached task and awaits
/// the task's result: the storage write and the invalidation land as one unit even if the calling
/// request is cancelled in between. An inline `write().await?; invalidate().await` pair leaves the
/// invalidation lost forever to a cancellation — every managed id is never reused, so the drifted
/// derived state (a stale quota-coordinator slot, a stale affinity handle) would outlive the row.
///
/// The mutation future must own everything it touches — the database handle, the caches, the
/// addressed ids — so the detached task never borrows from the request.
///
/// # Panics
///
/// Panics if the mutation task fails to hand off its result, a fail-fast task invariant.
async fn detach_mutation<T, F>(mutation: F) -> Result<T, Error>
where
    F: Future<Output = Result<T, Error>> + Send + 'static,
    T: Send + 'static,
{
    #[expect(
        clippy::expect_used,
        reason = "the mutation task always hands its result to the handler"
    )]
    tokio::spawn(mutation)
        .await
        .expect("the mutation task completes")
}

/// Validates a management request identifier. Identifier content — its characters, case, and length
/// — is the caller's choice; the backend enforces only structural non-emptiness
/// (`docs/reference/management-api.md`).
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

/// Validates a machine-consumed secret (`docs/reference/management-api.md`): gateway API key
/// secrets (`key`) and upstream static keys (`api_key`). The value must be non-empty printable
/// ASCII without whitespace — an empty value is [`Error::EmptyIdentifier`], any other composition
/// failure [`Error::InvalidIdentifier`].
///
/// # Errors
///
/// Returns [`Error`] when the value is empty or carries a character outside the allowed set.
fn validate_composition(param: &'static str, value: &str) -> Result<(), Error> {
    validate_routing_safe(param, value, crate::routing_text::is_routing_safe_text)
}

/// The shared body of the two composition validators: the non-empty check followed by the
/// caller-supplied routing-safety predicate. An empty value is [`Error::EmptyIdentifier`], any
/// other composition failure [`Error::InvalidIdentifier`].
///
/// # Errors
///
/// Returns [`Error`] when the value is empty or carries a character outside the allowed set.
fn validate_routing_safe(
    param: &'static str,
    value: &str,
    is_routing_safe: fn(&str) -> bool,
) -> Result<(), Error> {
    validate_identifier(param, value)?;

    if !is_routing_safe(value) {
        return Err(Error::InvalidIdentifier { param });
    }

    Ok(())
}

/// Validates an optional provider prefix at the management write boundary.
///
/// # Errors
///
/// Returns a validation error for a nonempty prefix outside the routing-safe token contract.
fn validate_model_prefix(value: &str) -> Result<(), Error> {
    if !value.is_empty() {
        validate_routing_token("model_prefix", value)?;
    }
    Ok(())
}

/// Validates exact upstream IDs and explicitly authored public route IDs as nonempty printable
/// ASCII without whitespace, `/`, or `:`. Every outward protocol can address such a token.
///
/// # Errors
///
/// Returns [`Error`] when the value is empty or carries a character outside the allowed set.
fn validate_routing_token(param: &'static str, value: &str) -> Result<(), Error> {
    validate_routing_safe(param, value, crate::routing_text::is_routing_safe_token)
}

/// Validates a static-key provider's base URL against the shared base-URL contract of
/// [`crate::routing_text::is_base_url`] (`docs/reference/management-api.md`): routing-safe text
/// and a non-empty `http://` or `https://` scheme. Provisioning applies the same contract to
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

/// Builds the rejection for a non-positive definition-version parameter — the one spelling every
/// path reading one shares: the definition-reference resolution prelude's
/// `provider_definition_version` (the provider write paths `providers:create` and
/// `providers:update`, and the OAuth flow paths `oauth:start` and `oauth:reauthorize` with an
/// explicitly named target) and `provider-definitions:update`'s `base_version`.
fn definition_version_invalid(param: &'static str) -> Error {
    Error::InvalidDefinition(crate::provider::definition::ValidationError::new(format!(
        "`{param}` must be a positive version number"
    )))
}

/// Parses a definition-version request parameter into its storage type. The version contract is
/// positivity — the schema's `CHECK version > 0` makes any non-positive version unrepresentable —
/// and [`NonZeroI64`] alone admits negatives, so every path reading a version gates through this
/// one parse: a non-positive value is the malformed-parameter rejection
/// ([`definition_version_invalid`]) shared by every such path, never a missing-reference
/// classification downstream.
///
/// # Errors
///
/// Returns [`definition_version_invalid`] for a non-positive version.
fn definition_version(version: i64, param: &'static str) -> Result<NonZeroI64, Error> {
    NonZeroI64::new(version)
        .filter(|version| version.is_positive())
        .ok_or_else(|| definition_version_invalid(param))
}

/// Builds the rejection for a definition rebind that names only one half of the
/// `(provider_definition_name, provider_definition_version)` reference pair — the one spelling
/// `providers:update`'s rebind fields and `oauth:reauthorize`'s explicit target share, because the
/// wire shape cannot express the pairing rule.
fn definition_rebind_invalid() -> Error {
    Error::InvalidDefinition(crate::provider::definition::ValidationError::new(
        "a definition rebind names both `provider_definition_name` and \
         `provider_definition_version`",
    ))
}

/// Resolves one definition reference (`docs/reference/management-api.md`): the version must be a
/// positive number and the exact `(name, version)` pair must exist — the latest version is never
/// substituted. A non-positive version renders [`definition_version_invalid`], an unknown pair
/// [`Error::DefinitionNotFound`].
///
/// # Errors
///
/// Returns [`Error`] for a non-positive version, an unknown definition pair, and storage
/// failures.
async fn resolve_definition(
    database: &crate::database::Database,
    name: &str,
    version: i64,
) -> Result<crate::provider::ProviderDefinition, Error> {
    let version = definition_version(version, "provider_definition_version")?;

    crate::storage::provider_definitions::find_definition(database, name, version)
        .await?
        .ok_or_else(|| Error::DefinitionNotFound {
            name: name.to_string(),
            version: version.get(),
        })
}

/// Builds the shared base-URL 400 template (`docs/design/storage.md` and
/// `docs/reference/management-api.md`): the effective definition's URLs reference the base, and
/// `tail` states why the calling path cannot satisfy the requirement — a request-supplied base
/// means the provider row must still carry one, a row-stored base means the row carries none.
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

/// The success body of a route whose operation produces no fact the caller cannot know
/// (`docs/reference/management-api.md` response ownership): an empty JSON object, shared by
/// reference like every other response shape.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
struct EmptyResponse {}

/// A provider row in its management wire shape: a union discriminated by the row's credential
/// kind — the one field set a row carries — with the exact definition pair, the derived wire
/// format, and the nullable base URL. Credentials are never returned. `id` is the provider's
/// public identifier — every management reference to an existing provider goes through
/// `provider_id` — while `name` is the unique, renameable label.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
enum ProviderResponse {
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
    /// Renders one provider row, consuming it; credentials are never read.
    #[must_use]
    fn from_provider(provider: crate::provider::Provider) -> Self {
        let shared = SharedProviderFields {
            id: provider.id,
            name: provider.name,
            model_prefix: provider.model_prefix,
            is_enabled: provider.is_enabled,
            provider_definition_name: provider.definition.name,
            provider_definition_version: provider.definition.version.get(),
            format: provider.definition.payload.format,
            base_url: provider.base_url,
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
struct SharedProviderFields {
    /// Provider row identifier.
    id: i64,

    /// Unique, renameable label.
    name: String,

    /// Prefix prepended to automatically exposed direct model names.
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

/// The `providers:create` success body (`docs/reference/management-api.md`); the response states
/// the server-assigned row identity — the fact the caller cannot know.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
struct ProviderEnvelope {
    /// The created provider row.
    provider: ProviderResponse,
}
