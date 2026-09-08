//! Outward client API routing and handling.

use super::database::Database;
use axum::Router;

/// Constructs the outward client API router.
pub(crate) fn router() -> Router<Database> {
    Router::new()
}
