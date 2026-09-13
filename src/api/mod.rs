//! Outward client API routing.

use crate::context::Context;
use axum::Router;

mod openai_v1;

/// Constructs the outward client API router, configured with CORS middleware.
///
/// The router defines no fallback of its own: unmapped paths under `/api`
/// propagate upward to the root router's empty 404, while the nested
/// `/openai-v1` sub-router owns the OpenAI-shaped fallback for its own paths.
pub(crate) fn router() -> Router<Context> {
    Router::new()
        .nest("/openai-v1", openai_v1::router())
        .layer(crate::CONFIG.api.cors.cors_layer())
}
