//! Administrative management API routing and handling.

use super::database::Database;
use axum::Router;

/// Constructs the administrative management API router.
pub(crate) fn router() -> Router<Database> {
    Router::new()
}
