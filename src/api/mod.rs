//! Outward client API routing: the `/api` parent router, the shared error domain every
//! format surface renders through its own wire shape (README §6.7), and the shared outward
//! serving pipeline every format surface runs through ([`serving`]).

use super::{context::Context, database::Database, server};
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
    // TODO(axum 0.9): drop this fallback and let the nested fallback of `/openai-v1` inherit from
    // the root router again. `Router::nest` in axum 0.8 discards a nested router's fallback —
    // together with every fallback collected from routers nested below it — when the router being
    // nested declares no fallback of its own (tokio-rs/axum#3138, fixed by tokio-rs/axum#3158 for
    // 0.9), which would leave that 404 unreachable. The declaration is deliberately the very
    // handler the root router uses, so an unmapped path is answered identically either way.
    Router::new()
        .nest("/openai-v1", openai_v1::router(database.clone()))
        .nest("/gemini-v1beta", gemini_v1beta::router(database.clone()))
        .nest("/anthropic-v1", anthropic_v1::router(database))
        .fallback(server::fallback)
        .layer(crate::CONFIG.api_cors_layer())
}
