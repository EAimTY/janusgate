//! Gemini inbound surfaces (`/api/gemini-v1beta`): GenerateContent and Interactions under one
//! prefix (README §6.3).
//!
//! This module owns the shared prefix router: `x-goog-api-key` authentication (header only, no
//! `?key=`), the format-compliant 404/405 bodies via [`crate::format::error_gemini`], the body
//! limit, and the single `alt=sse` streaming query predicate both endpoint modules enforce. The
//! shared outward serving pipeline (completion handling, responses, authentication middleware,
//! fallbacks) lives in [`crate::api::serving`]. Endpoint handlers live in [`generate_content`]
//! (Gemini GenerateContent surface) and [`interactions`] (Gemini Interactions surface).

use crate::{
    api::{Error, log_error, serving},
    context::Context,
    database::Database,
};
use axum::{
    Router,
    extract::{DefaultBodyLimit, Request},
    http::StatusCode,
    middleware,
};
use serde::Serialize;

pub(super) mod generate_content;
pub(super) mod interactions;

/// The Gemini prefix surface's identity for the shared pipeline: `x-goog-api-key` credentials
/// (header only — no `?key=` query form) and the Gemini error envelope.
struct Surface;

impl serving::Surface for Surface {
    fn render_error(error: &Error) -> (StatusCode, impl Serialize) {
        crate::format::error_gemini::render(error)
    }

    fn extract_credential(request: &Request) -> Result<String, serving::AuthRejection> {
        let Some(presented) = request
            .headers()
            .get("x-goog-api-key")
            .and_then(|value| value.to_str().ok())
        else {
            log_error("request", &Error::InvalidApiKey);

            return Err(serving::AuthRejection::MissingCredential);
        };

        Ok(presented.to_owned())
    }
}

/// Constructs the prefix router with Gemini-key authentication and its own 404/405 responses.
///
/// `database` feeds the authentication middleware its own state, so every request checks the
/// presented key against the live database rather than the router state.
pub(super) fn router(database: Database) -> Router<Context> {
    Router::new()
        .merge(generate_content::router())
        .merge(interactions::router())
        .fallback(serving::fallback::<Surface>)
        .method_not_allowed_fallback(serving::method_not_allowed::<Surface>)
        .layer(middleware::from_fn_with_state(
            database,
            serving::authenticate::<Surface>,
        ))
        .layer(DefaultBodyLimit::max(serving::MAX_BODY_BYTES))
}

/// Reports whether a query string carries `alt=sse`: the only supported streaming query form of
/// both Gemini endpoints. The check parses the raw query without the axum `query` extractor
/// (which would require the axum `query` feature) by splitting on `&` and `=`; no form decoding
/// is needed — the override is exactly `alt=sse`.
pub(super) fn has_alt_sse(query: Option<&str>) -> bool {
    query.is_some_and(|query| {
        query
            .split('&')
            .any(|pair| matches!(pair.split_once('='), Some(("alt", "sse"))))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the streaming query predicate: exactly the `alt=sse` pair matches, in any query
    /// position, while an absent query, an unrelated query, a prefixed name, a wrong value, a
    /// case difference, and a value carrying extra content all reject.
    ///
    /// # Panics
    ///
    /// Panics if a query classifies outside its documented outcome.
    #[test]
    fn detects_only_the_alt_sse_query_form() {
        for query in [
            "alt=sse",
            "alt=sse&other=1",
            "other=1&alt=sse",
            "other=1&alt=sse&more=2",
            // The override rides any `alt` pair: the first `alt=sse` forces streaming even when a
            // later pair would disagree.
            "alt=sse&alt=stream",
        ] {
            assert!(
                has_alt_sse(Some(query)),
                "`{query}` carries the streaming override"
            );
        }

        for query in [
            "other=1",
            "alt",
            "alt=",
            "alt=stream",
            "ALT=sse",
            "xalt=sse",
            "alt=sse=1",
        ] {
            assert!(
                !has_alt_sse(Some(query)),
                "`{query}` does not carry the streaming override"
            );
        }

        assert!(!has_alt_sse(None), "an absent query never streams");
    }
}
