//! Outward client API routing.

use crate::context::Context;
use axum::Router;
use tower_http::cors::CorsLayer;

mod openai_v1;

/// Constructs the outward client API router, configured with CORS middleware.
///
/// The router defines no fallback of its own: unmapped paths under `/api`
/// propagate upward to the root router's empty 404, while the nested
/// `/openai-v1` sub-router owns the OpenAI-shaped fallback for its own paths.
pub(crate) fn router() -> Router<Context> {
    Router::new().nest("/openai-v1", openai_v1::router()).layer(
        CorsLayer::new()
            .allow_origin(crate::CONFIG.api.cors.allowed_origin.clone())
            .allow_private_network(crate::CONFIG.api.cors.allow_private_network)
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any),
    )
}
