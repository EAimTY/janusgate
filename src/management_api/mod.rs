//! Administrative management API routing and handling.

use crate::{
    database::Database,
    error::{self, ApiError, log_api_error},
    format::UpstreamFormat,
    provider::Provider,
    security::constant_time_eq,
};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, OriginalUri, Request},
    http::header,
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use log::warn;
use serde::de::DeserializeOwned;
use tower_http::cors::CorsLayer;

pub(crate) mod models;
pub(crate) mod oauth_flow;
pub(crate) mod providers;

/// Maximum accepted request body size for the management API.
///
/// Management payloads are small JSON documents, so 2 MiB — axum's default,
/// made explicit here so the limit is deliberate and greppable — is generous;
/// the outward client surface allows 64 MiB for inline image data.
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Constructs the administrative management API router with its own token
/// authentication, 404 fallback, and CORS middleware.
pub(crate) fn router() -> Router<Database> {
    Router::new()
        .route("/list-providers", get(providers::list))
        .route("/save-provider", post(providers::save))
        .route("/delete-provider", post(providers::delete))
        .route("/start-provider-oauth", post(oauth_flow::start))
        .route("/complete-provider-oauth", post(oauth_flow::complete))
        .route("/discover-models", post(models::discover))
        .route("/list-models", get(models::list))
        .route("/save-model", post(models::save))
        .fallback(fallback)
        .layer(middleware::from_fn(authenticate))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(
            CorsLayer::new()
                .allow_origin(crate::CONFIG.management_api.cors.allowed_origin.clone())
                .allow_private_network(crate::CONFIG.management_api.cors.allow_private_network)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
}

/// Authenticates the request against the bearer-less management token.
async fn authenticate(request: Request, next: Next) -> Response {
    let is_authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| constant_time_eq(token, &crate::CONFIG.management_api.token));

    if !is_authorized {
        warn!("rejected an unauthorized management request");

        return error::json_response(
            401,
            ApiError::authentication("Missing or invalid management token").to_management_json(),
        );
    }

    next.run(request).await
}

/// Handles unmapped management paths with a management-shaped 404.
async fn fallback(OriginalUri(uri): OriginalUri, request: Request) -> Response {
    let error = ApiError::not_found(format!("Invalid URL ({} {})", request.method(), uri.path()));

    error::json_response(error.status, error.to_management_json())
}

/// Renders an error outcome into a management error response.
pub(crate) fn error_response(error: ApiError) -> Response {
    error::json_response(error.status, error.to_management_json())
}

/// Deserializes a strictly typed request body, logging a malformed body at
/// debug level as a rejection of the named `operation` (the rendered error
/// is the caller's only feedback; the log record is the server-side trace).
///
/// # Errors
///
/// Returns [`ApiError`] for malformed JSON and unknown fields.
pub(crate) fn parse_body<T>(body: &Bytes, operation: &str) -> Result<T, ApiError>
where
    T: DeserializeOwned,
{
    serde_json::from_slice(body).map_err(|error| {
        let api_error = error::invalid_request_from_json(error);

        log_api_error(&format!("{operation} request"), &api_error);

        api_error
    })
}

/// Validates a management request identifier. Identifier content — its
/// characters, case, and length — is the caller's choice; the backend
/// enforces only structural non-emptiness (README §7.1).
///
/// # Errors
///
/// Returns [`ApiError`] when the identifier is empty.
pub(crate) fn validate_identifier(param: &str, value: &str) -> Result<(), ApiError> {
    if value.is_empty() {
        return Err(ApiError::invalid_parameter(
            param,
            format!("`{param}` must not be empty"),
        ));
    }

    Ok(())
}

/// Renders a provider row into its management JSON shape; credentials are
/// never exposed. `id` is the provider's public identifier — every management
/// reference to an existing provider goes through it — while `name` is the
/// unique, renameable label.
pub(crate) fn provider_json(provider: &Provider) -> serde_json::Value {
    serde_json::json!({
        "id": provider.id,
        "name": provider.name,
        "model_prefix": provider.model_prefix,
        "type": provider.provider_type.as_str(),
        "protocol": provider.protocol.map(UpstreamFormat::as_str),
        "base_url": provider.base_url,
        "expires_at": provider.expires_at,
        "is_enabled": provider.is_enabled,
    })
}
