//! Outward client API routing and handling.

use crate::database::Database;
use axum::{
    Router,
    extract::{OriginalUri, Request},
    http::{StatusCode, header},
    response::Response,
};

pub(crate) mod openai_v1;

/// Constructs the outward client API router.
pub(crate) fn router() -> Router<Database> {
    Router::new()
        .nest("/openai-v1", openai_v1::router())
        .fallback(fallback)
}

/// Handles unmapped paths under `/api` with an API-scoped OpenAI-shaped 404.
async fn fallback(OriginalUri(uri): OriginalUri, request: Request) -> Response {
    let mut error = crate::error::ApiError::invalid_request(format!(
        "Invalid URL ({} {})",
        request.method(),
        uri.path()
    ));

    error.status = StatusCode::NOT_FOUND.as_u16();

    json_response(error.status, error.to_openai_json())
}

/// Builds a JSON response with the given status code.
///
/// # Panics
///
/// Panics if the static response parts fail to build, which cannot occur.
pub(crate) fn json_response(status: u16, body: serde_json::Value) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    #[expect(clippy::expect_used, reason = "JSON responses have known-valid parts")]
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .expect("static response parts are always valid")
}
