//! The tower-http CORS layers derived from the schema's CORS sections, and the live CORS accessors
//! of [`ConfigLoader`](super::ConfigLoader).

use super::{ConfigLoader, schema::CorsConfig};
use tower_http::cors::{Any, CorsLayer};

impl ConfigLoader {
    /// The CORS layer for the outward API sub-router (`/api`), built from the currently applied
    /// configuration. The routers are constructed once at startup, so this reflects the startup
    /// configuration; changing it requires a restart.
    pub(crate) fn api_cors_layer(&self) -> CorsLayer {
        cors_layer(&self.api.cors)
    }

    /// The CORS layer for the management API sub-router (`/management-api`); like
    /// [`ConfigLoader::api_cors_layer`] it reflects the startup configuration, because the routers
    /// are constructed once.
    pub(crate) fn management_api_cors_layer(&self) -> CorsLayer {
        cors_layer(&self.management_api.cors)
    }
}

/// Renders a CORS configuration into the tower-http CORS layer both HTTP sub-routers apply: the
/// configured origins and Private Network Access policy, with every request method and header
/// allowed.
fn cors_layer(cors: &CorsConfig) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(cors.allowed_origin.clone())
        .allow_private_network(cors.allow_private_network)
        .allow_methods(Any)
        .allow_headers(Any)
}
