//! Gemini inbound surfaces (`/api/gemini-v1beta`): GenerateContent and Interactions under one
//! prefix (`docs/reference/inference.md`).
//!
//! This module owns the shared prefix router: `x-goog-api-key` authentication (header only, no
//! `?key=`), the format-compliant 404/405 bodies via [`crate::api::error::gemini`], the body
//! limit, and the single `alt=sse` streaming query predicate both endpoint modules enforce. The
//! shared outward serving pipeline (completion handling, responses, authentication middleware,
//! fallbacks) lives in [`crate::api::serving`]. Endpoint handlers live in [`generate_content`]
//! (Gemini GenerateContent surface) and [`interactions`] (Gemini Interactions surface).

use crate::{
    api::{Error, serving},
    context::Context,
    database::Database,
};
use axum::{Router, extract::Request, http::StatusCode};
use serde::Serialize;

mod generate_content;
mod interactions;

#[cfg(test)]
mod tests;

/// The Gemini prefix surface's identity for the shared pipeline: `x-goog-api-key` credentials
/// (header only — no `?key=` query form) and the Gemini error envelope.
struct Surface;

impl serving::Surface for Surface {
    fn render_error(error: &Error) -> (StatusCode, impl Serialize) {
        super::error::gemini::render(error)
    }

    fn extract_credential(request: &Request) -> Result<String, serving::AuthRejection> {
        request
            .headers()
            .get("x-goog-api-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
            .ok_or(serving::AuthRejection::MissingCredential)
    }
}

/// Constructs the prefix router with Gemini-key authentication and its own 404/405 responses.
pub(super) fn router(database: Database) -> Router<Context> {
    serving::surface_router::<Surface>(
        database,
        Router::new()
            .merge(generate_content::router())
            .merge(interactions::router()),
    )
}

/// Reports whether a query string carries `alt=sse`: the only supported streaming query form of
/// both Gemini endpoints. The check parses the raw query without the axum `query` extractor
/// (which would require the axum `query` feature) by splitting on `&` and `=`; no form decoding
/// is needed — the override is exactly `alt=sse`.
fn has_alt_sse(query: Option<&str>) -> bool {
    query.is_some_and(|query| {
        query
            .split('&')
            .any(|pair| matches!(pair.split_once('='), Some(("alt", "sse"))))
    })
}
