//! The shared empty-404 router fallback: the one handler every routing domain with no route of
//! its own names.

use axum::http::StatusCode;

/// Answers a request that matches no route anywhere with an empty 404 response.
///
/// The root router owns this response, and the `/api` and `/management-api` parent routers declare
/// this very handler as their own fallback, so a path that belongs to no domain is answered
/// identically whichever router catches it.
///
/// TODO(axum 0.9): the parent declarations are an axum 0.8 workaround. `Router::nest` in axum 0.8
/// discards a nested router's fallback — together with every fallback collected from routers
/// nested below it — when the router being nested declares no fallback of its own
/// (tokio-rs/axum#3138, fixed by tokio-rs/axum#3158 for 0.9), so without the parents'
/// declarations the nested routers' 404s — the format sub-routers' under [`api::router`] and the
/// version routers' under [`management_api::router`] — would never run behind the fallback-free
/// parents. With axum 0.9, drop the declarations in [`api::router`] and
/// [`management_api::router`] and let their nested fallbacks inherit from the root router again;
/// the observable behavior is identical either way, because every router names this same handler.
pub(crate) async fn fallback() -> StatusCode {
    StatusCode::NOT_FOUND
}
