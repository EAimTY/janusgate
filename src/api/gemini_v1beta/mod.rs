//! Gemini inbound surfaces (`/api/gemini-v1beta`): GenerateContent and Interactions under one
//! prefix (REFACTOR §4).
//!
//! This module owns the shared prefix router: `x-goog-api-key` authentication (header only, no
//! `?key=`), the format-compliant 404/405 bodies via [`crate::format::error_gemini`], and the
//! body limit. Endpoint handlers live in the submodules owned by the format subagents:
//! [`generate_content`] (Gemini GenerateContent surface) and [`interactions`] (Gemini
//! Interactions surface).

use crate::{api::Error, context::Context, database::Database};
use axum::{
    Router,
    body::Body,
    extract::{DefaultBodyLimit, FromRequestParts, Request, State},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::Response,
};
use std::convert::Infallible;

pub(super) mod generate_content;
pub(super) mod interactions;

/// Maximum accepted request body size, matching the OpenAI surfaces (inline image data).
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Constructs the prefix router with Gemini-key authentication and its own 404/405 responses.
///
/// `database` feeds the authentication middleware its own state, so every request checks the
/// presented key against the live database rather than the router state.
pub(super) fn router(database: Database) -> Router<Context> {
    Router::new()
        .merge(generate_content::router())
        .merge(interactions::router())
        .fallback(fallback)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn_with_state(database, authenticate))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

/// The row id of the gateway API key that authorized the current request: the authentication
/// middleware stamps it into the request extensions when it lets a request through, and the
/// completion handlers extract it to attribute the request's accounting row. Only the
/// never-recycled row identity travels this far — never the secret itself.
#[derive(Debug, Clone, Copy)]
pub(super) struct AuthenticatedApiKeyRow(pub(super) i64);

impl<S> FromRequestParts<S> for AuthenticatedApiKeyRow
where
    S: Send + Sync,
{
    /// Never constructed: this extractor fails fast instead of rejecting. A completion handler
    /// always runs behind [`authenticate`] first, so a missing extension is an invariant
    /// violation of the router's own pipeline, not a client-repairable condition.
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        #[expect(
            clippy::expect_used,
            reason = "the authentication middleware always stamps this extension before a \
                      completion handler runs; a missing extension is an invariant violation \
                      that fails fast"
        )]
        let row_id = parts
            .extensions
            .get::<AuthenticatedApiKeyRow>()
            .expect("an authenticated completion request carries its API key row id")
            .0;

        Ok(Self(row_id))
    }
}

/// Authenticates the request against the enabled gateway API keys using the Gemini
/// `x-goog-api-key` header convention (header only — no `?key=` query form). The keys are read
/// from the live database, so a created, rotated, or disabled key takes effect on the next
/// request; a database failure renders the surface's internal error instead of an authentication
/// rejection.
async fn authenticate(State(database): State<Database>, request: Request, next: Next) -> Response {
    let presented = request
        .headers()
        .get("x-goog-api-key")
        .and_then(|value| value.to_str().ok());

    let Some(presented) = presented else {
        crate::api::log_error("request", &Error::InvalidApiKey);

        return error_response(Error::InvalidApiKey);
    };

    match crate::api_keys::list_enabled_api_key_secrets(&database).await {
        Ok(keys) => {
            let matched = keys
                .into_iter()
                .find(|(_, secret)| crate::security::constant_time_eq(presented, secret));

            match matched {
                Some((row_id, _secret)) => {
                    let mut request = request;

                    request
                        .extensions_mut()
                        .insert(AuthenticatedApiKeyRow(row_id));

                    next.run(request).await
                }

                None => {
                    crate::api::log_error("request", &Error::InvalidApiKey);

                    error_response(Error::InvalidApiKey)
                }
            }
        }

        Err(error) => {
            let error = Error::from(error);

            crate::api::log_error("request", &error);

            error_response(error)
        }
    }
}

/// Handles unmapped prefix paths with a Gemini-shaped 404, logging the rendered error once at
/// this response boundary.
async fn fallback(
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    request: Request,
) -> Response {
    let error = Error::InvalidUrl {
        method: request.method().as_str().to_string(),
        path: uri.path().to_string(),
    };

    crate::api::log_error("request", &error);

    error_response(error)
}

/// Handles a request whose method is not registered on an otherwise-matching path with a
/// Gemini-shaped 405, logging the rendered error once at this response boundary.
async fn method_not_allowed(method: Method, uri: axum::extract::OriginalUri) -> Response {
    let error = Error::MethodNotAllowed;

    crate::api::log_error(
        &format!("request {} {}", method.as_str(), uri.0.path()),
        &error,
    );

    error_response(error)
}

/// Renders one API failure as a Gemini JSON error response through the format's render module.
pub(super) fn error_response(error: Error) -> Response {
    let (status, envelope) = crate::format::error_gemini::render(&error);

    json_response(status, &envelope)
}

/// Builds a JSON response with the given status code from a typed payload, whose serialization
/// is total.
///
/// # Panics
///
/// Panics if the rendered body fails to serialize or the static response parts fail to build,
/// neither of which can occur.
pub(super) fn json_response<T>(status: StatusCode, body: &T) -> Response
where
    T: serde::Serialize,
{
    #[expect(
        clippy::expect_used,
        reason = "every outward body is a typed payload whose serialization is total"
    )]
    let rendered = serde_json::to_string(body).expect("outward response bodies serialize");

    #[expect(clippy::expect_used, reason = "JSON responses have known-valid parts")]
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(rendered))
        .expect("static response parts are always valid")
}
