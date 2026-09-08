//! Outward client API routing and handling.

use axum::Router;

/// Constructs the outward client API router.
pub(crate) fn router() -> Router {
    Router::new()
}
