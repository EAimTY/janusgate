//! Outward client API routing: the `/api` parent router, the shared error domain every
//! format surface renders through its own wire shape (`docs/reference/inference.md`), and the
//! shared outward serving pipeline every format surface runs through ([`serving`]).

use super::{context::Context, database::Database, router_fallback};
use axum::Router;

mod anthropic_v1;
pub(crate) mod error;
mod gemini_v1beta;
mod openai_v1;
mod serving;

pub(crate) use self::error::{Error, log_error};

/// Constructs the outward client API router, configured with CORS middleware.
///
/// The router defines no route of its own: it nests the format sub-routers and answers every other
/// path with the same empty 404 the root router renders. `database` feeds the format
/// sub-routers' authentication middleware.
pub(crate) fn router(database: Database) -> Router<Context> {
    // The shared empty 404 fallback; see `router_fallback::fallback` for the axum 0.9 rationale.
    Router::new()
        .nest("/openai-v1", openai_v1::router(database.clone()))
        .nest("/gemini-v1beta", gemini_v1beta::router(database.clone()))
        .nest("/anthropic-v1", anthropic_v1::router(database))
        .fallback(router_fallback::fallback)
        .layer(crate::CONFIG.api_cors_layer())
}
