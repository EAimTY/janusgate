//! Administrative management API routing.
//!
//! Management routes are versioned: each version is a submodule owning its routes, payloads, and
//! error domain, while the route contract, the `route!` macro, and the TypeScript bindings
//! generator are shared here. This module mounts the version routers under their own prefixes and
//! enumerates the versions wherever they must be enumerated.
//!
//! The router itself owns no route: paths under `/management-api` that belong to no version — the
//! version prefixes themselves included — are answered with the same empty 404 the root router
//! renders, while each version's router owns the error-shaped 404 for its own unmapped paths.

use crate::{context::Context, router_fallback};
use axum::Router;
#[cfg(feature = "management-api-typescript-bindings")]
use std::path::Path;

#[cfg(feature = "management-api-typescript-bindings")]
mod bindings;
mod route;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod tests_support;
mod v0;

use self::route::Route;

/// Generates every management API version's TypeScript bindings into one `<version>.ts` file per
/// version, directly in `output_target`, which may be relative or absolute and is created when
/// absent. Versions are listed explicitly: each contributes its prefix — which names its file — its
/// route registry and the payload types it publishes besides its routes.
///
/// # Panics
///
/// Panics when a version's registry holds two routes sharing one method and path, when two distinct
/// payload types of one version share one TypeScript name, or when the filesystem fails; the
/// generator is a first-party build tool and every failure is a fixable declaration or
/// environment bug.
#[cfg(feature = "management-api-typescript-bindings")]
pub fn generate_typescript_bindings(output_target: &Path) {
    bindings::generate(output_target, v0::PREFIX, &v0::ROUTES, v0::EXPORTED_TYPES);
}

/// Constructs the management API router, mounting every version under its prefix and applying the
/// cross-version CORS policy the `management_api.cors` configuration section configures.
pub(crate) fn router() -> Router<Context> {
    // The shared empty 404 fallback; see `router_fallback::fallback` for the axum 0.9 rationale.
    Router::new()
        .nest(v0::PREFIX, v0::router())
        .fallback(router_fallback::fallback)
        .layer(crate::CONFIG.management_api_cors_layer())
}
