//! Administrative management API routing and handling.

use crate::{
    database::Database, error::ApiError, format::UpstreamFormat, provider::Provider,
    security::constant_time_eq,
};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, OriginalUri, Request},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use serde::de::DeserializeOwned;

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
/// authentication and 404 fallback.
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
}

/// Authenticates the request against the bearer-less management token.
async fn authenticate(request: Request, next: Next) -> Response {
    let is_authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| constant_time_eq(token, &crate::CONFIG.management_api.token));

    if !is_authorized {
        return json_response(
            401,
            ApiError::authentication("Missing or invalid management token").to_management_json(),
        );
    }

    next.run(request).await
}

/// Handles unmapped management paths with a management-shaped 404.
async fn fallback(OriginalUri(uri): OriginalUri, request: Request) -> Response {
    let mut error =
        ApiError::not_found(format!("Invalid URL ({} {})", request.method(), uri.path()));

    error.status = StatusCode::NOT_FOUND.as_u16();

    json_response(error.status, error.to_management_json())
}

/// Builds a JSON response with the given status code.
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
        .body(axum::body::Body::from(body.to_string()))
        .expect("static response parts are always valid")
}

/// Renders an error outcome into a management error response.
pub(crate) fn error_response(error: ApiError) -> Response {
    json_response(error.status, error.to_management_json())
}

/// Deserializes a strictly typed request body.
///
/// # Errors
///
/// Returns [`ApiError`] for malformed JSON and unknown fields.
pub(crate) fn parse_body<T: DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(crate::error::invalid_request_from_json)
}

/// Renders a provider row into its management JSON shape; credentials are
/// never exposed.
pub(crate) fn provider_json(provider: &Provider) -> serde_json::Value {
    serde_json::json!({
        "name": provider.name,
        "model_prefix": provider.model_prefix,
        "type": provider.provider_type.as_str(),
        "protocol": provider.protocol.map(UpstreamFormat::as_str),
        "base_url": provider.base_url,
        "expires_at": provider.expires_at,
        "enabled": provider.enabled,
    })
}
