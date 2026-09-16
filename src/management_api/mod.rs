//! Administrative management API routing, its self-contained error domain,
//! and handling.

#[cfg(feature = "management-api-typescript-bindings")]
mod bindings;
mod error;
mod models;
mod oauth_flow;
mod providers;
mod route;

use self::error::ManagementApiError;
// The management API's public route surface: the route contract, every route
// marker, and every route's request and response payload types. Nested wire
// shapes — provider and model rows, captured credentials — stay private.
pub use crate::ir::IrReasoningEffort;
use crate::{context::Context, format::UpstreamFormat, provider::Provider, security};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, OriginalUri, Request},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::{MethodFilter, on},
};
#[cfg(feature = "management-api-typescript-bindings")]
pub use bindings::generate_typescript_bindings;
use log::warn;
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
pub use route::Route;
use route::route;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

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
        clippy::panic,
        reason = "management routes declare standard HTTP methods only"
    )]
    MethodFilter::try_from(method.clone())
        .unwrap_or_else(|rejection| panic!("non-standard HTTP method {rejection}"))
}

/// Constructs the administrative management API router with its own token
/// authentication, 404 fallback, and CORS middleware. Method and path are
/// wired from each route's [`Route`] constants, so the `route!` declaration
/// is the single place they are written.
pub(crate) fn router() -> Router<Context> {
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
        .layer(middleware::from_fn(authenticate))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(crate::CONFIG.management_api.cors.cors_layer())
}

/// Authenticates the request against the bearer-less management token.
async fn authenticate(request: Request, next: Next) -> Response {
    let is_authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| {
            security::constant_time_eq(token, &crate::CONFIG.management_api.token)
        });

    if !is_authorized {
        warn!("rejected an unauthorized management request");

        return ManagementApiError::authentication("Missing or invalid management token")
            .into_response();
    }

    next.run(request).await
}

/// Handles unmapped management paths with a management-shaped 404, logging
/// the rendered error once at this response boundary.
async fn fallback(OriginalUri(uri): OriginalUri, request: Request) -> Response {
    let error =
        ManagementApiError::not_found(format!("Invalid URL ({} {})", request.method(), uri.path()));

    error::log_error("request", &error);

    error.into_response()
}

/// Deserializes a strictly typed request body, logging a malformed body at
/// debug level as a rejection of the named `operation` (the rendered error
/// is the caller's only feedback; the log record is the server-side trace).
///
/// # Errors
///
/// Returns [`ManagementApiError`] for malformed JSON and unknown fields.
fn parse_body<T>(body: &Bytes, operation: &str) -> Result<T, ManagementApiError>
where
    T: DeserializeOwned,
{
    serde_json::from_slice(body).map_err(|error| {
        let management_error =
            ManagementApiError::invalid_request(format!("Invalid JSON body: {error}"));

        error::log_error(&format!("{operation} request"), &management_error);

        management_error
    })
}

/// Validates a management request identifier. Identifier content — its
/// characters, case, and length — is the caller's choice; the backend
/// enforces only structural non-emptiness (README §7.1).
///
/// # Errors
///
/// Returns [`ManagementApiError`] when the identifier is empty.
fn validate_identifier(param: &str, value: &str) -> Result<(), ManagementApiError> {
    if value.is_empty() {
        return Err(ManagementApiError::invalid_request(format!(
            "`{param}` must not be empty"
        )));
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

/// Builds a JSON response with the given status code.
///
/// The management surface's universal response builder, defined at the
/// surface root and kept separate from the outward one so each router owns
/// its rendering. The body is a typed payload, so serialization is total.
///
/// # Panics
///
/// Panics if the rendered body fails to serialize or the static response
/// parts fail to build, neither of which can occur.
fn json_response<T>(status: StatusCode, body: &T) -> Response
where
    T: Serialize,
{
    #[expect(
        clippy::expect_used,
        reason = "every management body is a typed payload whose serialization is total"
    )]
    let rendered = serde_json::to_string(body).expect("management response bodies serialize");

    #[expect(clippy::expect_used, reason = "JSON responses have known-valid parts")]
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(rendered))
        .expect("static response parts are always valid")
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
