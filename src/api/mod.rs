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
use tower_http::cors::CorsLayer;

pub(crate) mod openai_v1;

// The format sub-router renders through the shared JSON response builder;
// re-exporting it here keeps the child on its `super::` import path.
pub(crate) use crate::error::json_response;

/// Constructs the outward client API router, configured with CORS middleware.
pub(crate) fn router() -> Router<Database> {
    Router::new()
        .nest("/openai-v1", openai_v1::router())
        .fallback(fallback)
        .layer(
            CorsLayer::new()
                .allow_origin(crate::CONFIG.api.cors.allowed_origin.clone())
                .allow_private_network(crate::CONFIG.api.cors.allow_private_network)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
}

/// Handles unmapped paths under `/api` with an API-scoped OpenAI-shaped 404.
async fn fallback(OriginalUri(uri): OriginalUri, request: Request) -> Response {
    let mut error =
        ApiError::invalid_request(format!("Invalid URL ({} {})", request.method(), uri.path()));

    error.status = StatusCode::NOT_FOUND.as_u16();

    error::json_response(error.status, error.to_openai_json())
}

#[cfg(test)]
mod tests {
    use super::router;
    use crate::{
        database::Database,
        test_support::{self, TempDatabaseGuard},
    };
    use axum::{
        body::Body,
        http::{Request as HttpRequest, StatusCode},
    };
    use std::{error::Error as StdError, time::Duration};
    use tower::ServiceExt as _;

    /// Opens an isolated guarded test database and returns it alongside the
    /// API router with that database as state and its deletion guard.
    ///
    /// # Panics
    ///
    /// Panics if the global test configuration is not yet initializable.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while opening the test database.
    async fn test_router() -> Result<(Database, axum::Router, TempDatabaseGuard), Box<dyn StdError>>
    {
        test_support::init_config();

        let (database, guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let router = router().with_state(database.clone());

        Ok((database, router, guard))
    }

    /// Verifies that CORS preflight on format sub-routes succeeds without
    /// client credentials and returns the configured CORS headers.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the router.
    #[tokio::test]
    async fn handles_cors_preflight_on_nested_format_sub_router() -> Result<(), Box<dyn StdError>> {
        let (_, router, _guard) = test_router().await?;

        #[expect(clippy::expect_used, reason = "a build failure fails the test")]
        let request = HttpRequest::builder()
            .method("OPTIONS")
            .uri("/openai-v1/chat/completions")
            .header("origin", "https://chat.example.com")
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "authorization, content-type",
            )
            .header("access-control-request-private-network", "true")
            .body(Body::empty())
            .expect("request should build");

        // SAFETY: not applicable; in-memory oneshot cannot fail.
        let response = match router.oneshot(request).await {
            Ok(response) => response,
            Err(error) => unreachable!("in-memory oneshot cannot fail: {error}"),
        };

        assert_eq!(response.status(), StatusCode::OK);

        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .and_then(|value| value.to_str().ok()),
            Some("*")
        );

        assert!(
            response
                .headers()
                .contains_key("access-control-allow-methods")
        );

        assert!(
            response
                .headers()
                .contains_key("access-control-allow-headers")
        );

        assert_eq!(
            response
                .headers()
                .get("access-control-allow-private-network")
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );

        Ok(())
    }

    /// Verifies that CORS preflight also applies to unmapped paths directly
    /// under `/api`, proving the CORS layer is mounted on the `/api` router.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the router.
    #[tokio::test]
    async fn handles_cors_preflight_on_unmapped_api_path() -> Result<(), Box<dyn StdError>> {
        let (_, router, _guard) = test_router().await?;

        #[expect(clippy::expect_used, reason = "a build failure fails the test")]
        let request = HttpRequest::builder()
            .method("OPTIONS")
            .uri("/definitely-unmapped")
            .header("origin", "https://chat.example.com")
            .header("access-control-request-method", "GET")
            .body(Body::empty())
            .expect("request should build");

        // SAFETY: not applicable; in-memory oneshot cannot fail.
        let response = match router.oneshot(request).await {
            Ok(response) => response,
            Err(error) => unreachable!("in-memory oneshot cannot fail: {error}"),
        };

        assert_eq!(response.status(), StatusCode::OK);

        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .and_then(|value| value.to_str().ok()),
            Some("*")
        );

        Ok(())
    }

    /// Verifies that actual requests to outward format endpoints receive
    /// CORS response headers.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the router.
    #[tokio::test]
    async fn attaches_cors_headers_to_api_responses() -> Result<(), Box<dyn StdError>> {
        let (_, router, _guard) = test_router().await?;

        // Authenticated request:
        #[expect(clippy::expect_used, reason = "a build failure fails the test")]
        let request = HttpRequest::builder()
            .method("GET")
            .uri("/openai-v1/models")
            .header("origin", "https://chat.example.com")
            .header("authorization", "Bearer test-api-key")
            .body(Body::empty())
            .expect("request should build");

        // SAFETY: not applicable; in-memory oneshot cannot fail.
        let response = match router.clone().oneshot(request).await {
            Ok(response) => response,
            Err(error) => unreachable!("in-memory oneshot cannot fail: {error}"),
        };

        assert_eq!(response.status(), StatusCode::OK);

        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .and_then(|value| value.to_str().ok()),
            Some("*")
        );

        // Unauthorized request:
        #[expect(clippy::expect_used, reason = "a build failure fails the test")]
        let unauthorized_request = HttpRequest::builder()
            .method("GET")
            .uri("/openai-v1/models")
            .header("origin", "https://chat.example.com")
            .body(Body::empty())
            .expect("request should build");

        // SAFETY: not applicable; in-memory oneshot cannot fail.
        let unauthorized_response = match router.oneshot(unauthorized_request).await {
            Ok(response) => response,
            Err(error) => unreachable!("in-memory oneshot cannot fail: {error}"),
        };

        assert_eq!(unauthorized_response.status(), StatusCode::UNAUTHORIZED);

        assert_eq!(
            unauthorized_response
                .headers()
                .get("access-control-allow-origin")
                .and_then(|value| value.to_str().ok()),
            Some("*")
        );

        Ok(())
    }
}
