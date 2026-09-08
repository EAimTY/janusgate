//! Outward client API routing and handling.

use crate::database::Database;
use axum::Router;

/// Constructs the outward client API router.
pub(crate) fn router() -> Router<Database> {
    Router::new()
}
