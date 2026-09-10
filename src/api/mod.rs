//! Outward client API routing and handling.

use crate::{
    database::Database,
    error::{self, ApiError},
};
use axum::{
    Router,
    extract::{OriginalUri, Request},
    http::StatusCode,
    response::Response,
};

pub(crate) mod openai_v1;

// The format sub-router renders through the shared JSON response builder;
// re-exporting it here keeps the child on its `super::` import path.
pub(crate) use crate::error::json_response;

/// Constructs the outward client API router.
pub(crate) fn router() -> Router<Database> {
    Router::new()
        .nest("/openai-v1", openai_v1::router())
        .fallback(fallback)
}

/// Handles unmapped paths under `/api` with an API-scoped OpenAI-shaped 404.
async fn fallback(OriginalUri(uri): OriginalUri, request: Request) -> Response {
    let mut error =
        ApiError::invalid_request(format!("Invalid URL ({} {})", request.method(), uri.path()));

    error.status = StatusCode::NOT_FOUND.as_u16();

    error::json_response(error.status, error.to_openai_json())
}
