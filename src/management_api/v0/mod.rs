//! Management API version 0: its own route surface, self-contained error
//! domain, and handling.
//!
//! A management API version owns everything that can break between versions —
//! its routes, its request and response payloads, and its error taxonomy —
//! and nothing else: the management API module mounts this router at
//! [`PREFIX`] and applies the cross-version CORS policy, while authentication,
//! the body limit, and the errno-shaped 404 fallback belong to the version
//! itself.
//!
//! Handlers follow axum's own shape: a request body extracts through
//! `Json` wrapped in `WithRejection` with [`Error`] as the rejection
//! type, and every handler returns `Result<Json<..>, Error>`, so a failure
//! leaves through the one [`IntoResponse`] boundary that logs and renders
//! it — no handler matches on errors or logs them itself.

mod error;
mod models;
mod oauth_flow;
mod providers;

use self::error::Error;
// This version's public wire surface: every route marker and every route's
// request and response payload types. Nested wire shapes — provider and model
// rows, captured credentials — stay private, and the route contract itself is
// shared at [`crate::management_api::Route`].
pub use crate::ir::IrReasoningEffort;
#[cfg(feature = "management-api-typescript-bindings")]
use crate::management_api::route::RouteBinding;
#[cfg(feature = "management-api-typescript-bindings")]
use crate::management_api::route::{PayloadSource, payload_binding};
use crate::{
    context::Context, format::UpstreamFormat, management_api::Route, provider::Provider, security,
};
use axum::{
    Router,
    extract::{DefaultBodyLimit, OriginalUri, Request},
    http::{Method, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{MethodFilter, on},
};
pub use error::{ErrorCode, ErrorResponse};
pub use models::{
    DiscoverModels, DiscoverModelsRequest, DiscoverModelsResponse, ListModels, ListModelsRequest,
    ListModelsResponse, ModelEnvelope, ModelResponse, SaveModel, SaveModelRequest,
};
pub use oauth_flow::{
    CompleteProviderOauth, CompleteProviderOauthRequest, StartProviderOauth,
    StartProviderOauthRequest, StartProviderOauthResponse,
};
pub use providers::{
    DeleteProvider, DeleteProviderRequest, DeleteProviderResponse, ListProviders,
    ListProvidersRequest, ListProvidersResponse, SaveProvider, SaveProviderCredentials,
    SaveProviderRequest,
};
use serde::{Deserialize, Serialize};

/// URL prefix this version is mounted at, relative to the management API
/// router. Its version segment also names this version's TypeScript bindings
/// file.
pub const PREFIX: &str = "/v0";

/// Every route of this version, collected link-time; iteration order is
/// unspecified, so the bindings generator sorts before rendering. This
/// version's `route!` macro registers into it, and the management API module
/// reads it for bindings generation.
#[cfg(feature = "management-api-typescript-bindings")]
#[linkme::distributed_slice]
pub(super) static ROUTES: [RouteBinding];

/// Payload types this version's bindings publish besides its route payloads:
/// the error body, which carries the errno taxonomy this version pins.
#[cfg(feature = "management-api-typescript-bindings")]
pub(super) static EXPORTED_TYPES: &[PayloadSource] = &[payload_binding::<ErrorResponse>];

/// Declares one version 0 route, registering it in [`ROUTES`]. The shared
/// `route!` macro carries the declaration syntax and the contract a route
/// must satisfy; this wrapper binds the version's registry, so a declaration
/// names neither the registry nor the version.
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
/// Management payloads are small JSON documents, so 2 MiB — axum's default,
/// made explicit here so the limit is deliberate and greppable — is generous;
/// the outward client surface allows 64 MiB for inline image data.
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Converts a route's method constant into axum's method filter.
///
/// # Panics
///
/// Panics when the method is not a standard HTTP method; management routes
/// declare standard methods only, so this is a route-declaration invariant.
fn method_filter(method: &http::Method) -> MethodFilter {
    #[expect(
        clippy::expect_used,
        reason = "management routes declare standard HTTP methods only"
    )]
    MethodFilter::try_from(method.clone()).expect("non-standard HTTP method")
}

/// Constructs this version's router with its own token authentication, 404
/// response, and body limit. Method and path are wired from each route's
/// [`Route`] constants, so the `route!` declaration is the single place they
/// are written.
///
/// The router defines no CORS policy of its own: the management API router
/// applies the cross-version one around it.
pub(super) fn router() -> Router<Context> {
    Router::new()
        .route(
            providers::ListProviders::PATH,
            on(
                method_filter(&providers::ListProviders::METHOD),
                providers::list,
            ),
        )
        .route(
            providers::SaveProvider::PATH,
            on(
                method_filter(&providers::SaveProvider::METHOD),
                providers::save,
            ),
        )
        .route(
            providers::DeleteProvider::PATH,
            on(
                method_filter(&providers::DeleteProvider::METHOD),
                providers::delete,
            ),
        )
        .route(
            oauth_flow::StartProviderOauth::PATH,
            on(
                method_filter(&oauth_flow::StartProviderOauth::METHOD),
                oauth_flow::start,
            ),
        )
        .route(
            oauth_flow::CompleteProviderOauth::PATH,
            on(
                method_filter(&oauth_flow::CompleteProviderOauth::METHOD),
                oauth_flow::complete,
            ),
        )
        .route(
            models::DiscoverModels::PATH,
            on(
                method_filter(&models::DiscoverModels::METHOD),
                models::discover,
            ),
        )
        .route(
            models::ListModels::PATH,
            on(method_filter(&models::ListModels::METHOD), models::list),
        )
        .route(
            models::SaveModel::PATH,
            on(method_filter(&models::SaveModel::METHOD), models::save),
        )
        .fallback(fallback)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn(authenticate))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

/// Authenticates the request against the bearer-less management token. A
/// rejected request renders [`Error::Unauthorized`] through the shared
/// boundary, which logs it.
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

/// Handles a request whose method is not registered on an otherwise-matching
/// path with the version's error-shaped 405. axum still attaches the `Allow`
/// header listing the registered methods.
async fn method_not_allowed(method: Method, OriginalUri(uri): OriginalUri) -> Error {
    Error::MethodNotAllowed {
        method,
        path: uri.path().to_string(),
    }
}

/// Handles this version's unmapped paths with the version's error-shaped
/// 404.
async fn fallback(method: Method, OriginalUri(uri): OriginalUri) -> Error {
    Error::UnmappedPath {
        method,
        path: uri.path().to_string(),
    }
}

/// Validates a management request identifier. Identifier content — its
/// characters, case, and length — is the caller's choice; the backend
/// enforces only structural non-emptiness (README §7).
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

/// A provider row in its management wire shape (README §7.1); credentials are
/// never exposed. `id` is the provider's public identifier — every management
/// reference to an existing provider goes through it — while `name` is the
/// unique, renameable label.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ProviderResponse {
    /// Provider row identifier.
    pub id: i64,

    /// Unique, renameable label.
    pub name: String,

    /// Prefix prepended to every public model name.
    pub model_prefix: String,

    /// Account kind.
    #[serde(rename = "type")]
    pub provider_type: String,

    /// Upstream wire format; `null` when the account kind pins it.
    pub protocol: Option<String>,

    /// Upstream base URL; `null` for OAuth accounts.
    pub base_url: Option<String>,

    /// Unix timestamp when the OAuth access token expires.
    pub expires_at: Option<i64>,

    /// Whether the provider serves traffic.
    pub is_enabled: bool,
}

impl ProviderResponse {
    /// Renders one provider row; credentials are never read.
    fn from_provider(provider: &Provider) -> Self {
        Self {
            id: provider.id,
            name: provider.name.clone(),
            model_prefix: provider.model_prefix.clone(),
            provider_type: provider.provider_type.as_str().to_string(),
            protocol: provider
                .protocol
                .map(UpstreamFormat::as_str)
                .map(String::from),
            base_url: provider.base_url.clone(),
            expires_at: provider.expires_at,
            is_enabled: provider.is_enabled,
        }
    }
}

/// The `save-provider` and `complete-provider-oauth` success body
/// (README §7.1); the envelope every provider-returning endpoint renders.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub struct ProviderEnvelope {
    /// The created or updated provider row.
    pub provider: ProviderResponse,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Credentials, ProviderType};

    /// Verifies that a rendered provider carries the exact management wire
    /// shape README §7.1 documents: the renamed `type` key, the snake_case
    /// keys, `null` for absent optional columns, and no credentials. Pinned as
    /// a rendered string, so a renamed field, a leaked credential, or a
    /// null/omission change fails here rather than at a client.
    ///
    /// # Panics
    ///
    /// Panics if the rendered provider diverges from the documented shape.
    #[test]
    fn renders_the_documented_provider_shape() {
        let provider = Provider {
            id: 7,
            name: "acme".to_string(),
            model_prefix: "acme/".to_string(),
            provider_type: ProviderType::OpenAiCompatible,
            protocol: Some(UpstreamFormat::OpenAiChatCompletion),
            base_url: Some("https://upstream.example".to_string()),
            credentials: Credentials::ApiKey("sk-secret".to_string()),
            expires_at: None,
            is_enabled: true,
        };

        #[expect(clippy::expect_used, reason = "the provider body always serializes")]
        let rendered = serde_json::to_string(&ProviderEnvelope {
            provider: ProviderResponse::from_provider(&provider),
        })
        .expect("the provider body serializes");

        assert_eq!(
            rendered,
            concat!(
                r#"{"provider":{"id":7,"name":"acme","model_prefix":"acme/","#,
                r#""type":"openai_compatible","protocol":"openai_chat_completion","#,
                r#""base_url":"https://upstream.example","expires_at":null,"is_enabled":true}}"#,
            )
        );
    }
}
