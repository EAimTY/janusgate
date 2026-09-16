//! Administrative management API routing.
//!
//! Management routes are versioned: each version is a submodule owning its
//! routes, payloads, and error domain, while the route contract, the `route!`
//! macro, and the TypeScript bindings generator are shared here. This module
//! mounts the version routers under their own prefixes and enumerates the
//! versions wherever they must be enumerated.
//!
//! The router itself owns no route: paths under `/management-api` that belong
//! to no version — the version prefixes themselves included — are answered
//! with the same empty 404 the root router renders, while each version's
//! router owns the error-shaped 404 for its own unmapped paths.

#[cfg(feature = "management-api-typescript-bindings")]
mod bindings;
mod route;
pub mod v0;

use crate::{context::Context, server::fallback};
use axum::Router;
pub use route::Route;
#[cfg(feature = "management-api-typescript-bindings")]
use std::path::Path;

/// Generates every management API version's TypeScript bindings into one
/// `<version>.ts` file per version, directly in `output_target`, which may be
/// relative or absolute and is created when absent. Versions are listed
/// explicitly: each contributes its prefix — which names its file — its route
/// registry, the payload types it publishes besides its routes, and the
/// ordered vocabularies whose variant order it publishes.
///
/// # Panics
///
/// Panics when a version's registry holds two routes sharing one method and
/// path, when two distinct payload types of one version share one TypeScript
/// name, when an ordered vocabulary's declaration is not a unit-only enum's
/// union, or when the filesystem fails; the generator is a first-party build
/// tool and every failure is a fixable declaration or environment bug.
#[cfg(feature = "management-api-typescript-bindings")]
pub fn generate_typescript_bindings(output_target: &Path) {
    bindings::generate(
        output_target,
        v0::PREFIX,
        &v0::ROUTES,
        v0::EXPORTED_TYPES,
        v0::ORDERED_VOCABULARIES,
    );
}

/// Constructs the management API router, mounting every version under its
/// prefix and applying the cross-version CORS policy the
/// `management_api.cors` configuration section configures.
pub(crate) fn router() -> Router<Context> {
    // TODO(axum 0.9): drop this fallback and let the version routers' fallbacks
    // inherit from the root router again. `Router::nest` in axum 0.8 discards a
    // nested router's fallback — together with every fallback collected from
    // routers nested below it — when the router being nested declares no
    // fallback of its own (tokio-rs/axum#3138, fixed by tokio-rs/axum#3158 for
    // 0.9), which would leave v0's 404 unreachable behind this fallback-free
    // parent. The declaration is deliberately the very handler the root router
    // uses, so an unmapped path is answered identically either way.
    Router::new()
        .nest(v0::PREFIX, v0::router())
        .fallback(fallback)
        .layer(crate::CONFIG.management_api.cors.cors_layer())
}
