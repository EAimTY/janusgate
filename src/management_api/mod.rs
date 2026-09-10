//! Administrative management API routing and handling.

use crate::{
    database::Database,
    error::{self, ApiError},
    format::UpstreamFormat,
    provider::Provider,
    security::constant_time_eq,
};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, OriginalUri, Request},
    http::header,
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use serde::de::DeserializeOwned;

pub(crate) mod models;
pub(crate) mod oauth_flow;
pub(crate) mod providers;

/// Maximum accepted request body size for the management API.
///
/// Management payloads are small JSON documents, so 2 MiB — axum's default,
/// made explicit here so the limit is deliberate and greppable — is generous;
/// the outward client surface allows 64 MiB for inline image data.
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Constructs the administrative management API router with its own token
/// authentication and 404 fallback.
pub(crate) fn router() -> Router<Database> {
    Router::new()
        .route("/list-providers", get(providers::list))
        .route("/save-provider", post(providers::save))
        .route("/delete-provider", post(providers::delete))
        .route("/start-provider-oauth", post(oauth_flow::start))
        .route("/complete-provider-oauth", post(oauth_flow::complete))
        .route("/discover-models", post(models::discover))
        .route("/list-models", get(models::list))
        .route("/save-model", post(models::save))
        .fallback(fallback)
        .layer(middleware::from_fn(authenticate))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

/// Authenticates the request against the bearer-less management token.
async fn authenticate(request: Request, next: Next) -> Response {
    let is_authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|token| constant_time_eq(token, &crate::CONFIG.management_api.token));

    if !is_authorized {
        return error::json_response(
            401,
            ApiError::authentication("Missing or invalid management token").to_management_json(),
        );
    }

    next.run(request).await
}

/// Handles unmapped management paths with a management-shaped 404.
async fn fallback(OriginalUri(uri): OriginalUri, request: Request) -> Response {
    let error = ApiError::not_found(format!("Invalid URL ({} {})", request.method(), uri.path()));

    error::json_response(error.status, error.to_management_json())
}

/// Renders an error outcome into a management error response.
pub(crate) fn error_response(error: ApiError) -> Response {
    error::json_response(error.status, error.to_management_json())
}

/// Deserializes a strictly typed request body.
///
/// # Errors
///
/// Returns [`ApiError`] for malformed JSON and unknown fields.
pub(crate) fn parse_body<T>(body: &Bytes) -> Result<T, ApiError>
where
    T: DeserializeOwned,
{
    serde_json::from_slice(body).map_err(error::invalid_request_from_json)
}

/// Renders a provider row into its management JSON shape; credentials are
/// never exposed.
pub(crate) fn provider_json(provider: &Provider) -> serde_json::Value {
    serde_json::json!({
        "name": provider.name,
        "model_prefix": provider.model_prefix,
        "type": provider.provider_type.as_str(),
        "protocol": provider.protocol.map(UpstreamFormat::as_str),
        "base_url": provider.base_url,
        "expires_at": provider.expires_at,
        "enabled": provider.is_enabled,
    })
}

#[cfg(test)]
mod tests {
    use super::router;
    use crate::{
        database::Database,
        provider::{ProviderType, storage},
        test_support::{self, TempDatabaseGuard},
    };
    use axum::{
        body::{Body, to_bytes},
        http::{Request as HttpRequest, StatusCode},
    };
    use serde_json::Value;
    use std::{error::Error as StdError, time::Duration};
    use tower::ServiceExt as _;

    /// The management token the shared test configuration installs.
    const MANAGEMENT_TOKEN: &str = "test-mgmt";

    /// Opens an isolated guarded test database and returns it alongside the
    /// management router with that database as state, so tests can seed
    /// rows without leaking the temporary file.
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

    /// Sends a request through the router and returns status and body.
    ///
    /// # Panics
    ///
    /// Panics if the request cannot be built or the in-memory oneshot serve
    /// fails, which only a router construction bug can cause.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while building the request or reading
    /// the response body.
    async fn send(
        router: axum::Router,
        method: &str,
        uri: &str,
        authorization: Option<&str>,
        body: Body,
    ) -> Result<(StatusCode, String), Box<dyn StdError>> {
        let mut builder = HttpRequest::builder().method(method).uri(uri);

        if let Some(authorization) = authorization {
            builder = builder.header("authorization", authorization);
        }

        #[expect(clippy::expect_used, reason = "a build failure fails the test")]
        let request = builder.body(body).expect("request should build");

        // SAFETY: not applicable; the in-memory oneshot serve is infallible,
        // so the unreachable arm documents the invariant.
        let response = match router.oneshot(request).await {
            Ok(response) => response,
            Err(error) => unreachable!("in-memory oneshot cannot fail: {error}"),
        };

        let status = response.status();

        let body = to_bytes(response.into_body(), 1 << 20).await?;

        Ok((status, String::from_utf8_lossy(&body).to_string()))
    }

    /// Parses a rendered response body into its JSON value.
    ///
    /// # Panics
    ///
    /// Panics if the body is not JSON, which fails the test.
    fn body_json(body: &str) -> Value {
        #[expect(clippy::expect_used, reason = "every response body is JSON")]
        serde_json::from_str(body).expect("the response body should be JSON")
    }

    /// Reads one nested `{"error": {...}}` field out of a rendered body.
    fn error_field<'a>(rendered: &'a Value, field: &str) -> &'a Value {
        rendered
            .get("error")
            .and_then(|error| error.get(field))
            .unwrap_or(&Value::Null)
    }

    /// Verifies the management router authenticates every request with the
    /// bearer-less token before any handler runs: a missing or wrong token
    /// yields 401 with `code: "unauthorized"`, and the correct token serves
    /// the endpoint (README §4, §7).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the router.
    #[tokio::test]
    async fn authenticates_management_requests_with_bearer_less_token()
    -> Result<(), Box<dyn StdError>> {
        let (_, router, _guard) = test_router().await?;

        for authorization in [None, Some("wrong-token"), Some("Bearer test-mgmt")] {
            let (status, body) = send(
                router.clone(),
                "GET",
                "/list-providers",
                authorization,
                Body::empty(),
            )
            .await?;

            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{authorization:?} must not authorize"
            );

            let rendered = body_json(&body);

            assert_eq!(error_field(&rendered, "code"), "unauthorized");
        }

        let (status, body) = send(
            router,
            "GET",
            "/list-providers",
            Some(MANAGEMENT_TOKEN),
            Body::empty(),
        )
        .await?;

        assert_eq!(status, StatusCode::OK);

        assert_eq!(body, r#"{"providers":[]}"#);

        Ok(())
    }

    /// Verifies unmapped management paths yield the management-shaped 404
    /// with `code: "not_found"` instead of the root router's empty body
    /// (README §2.2, §4).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the router.
    #[tokio::test]
    async fn fallback_returns_management_404_shape() -> Result<(), Box<dyn StdError>> {
        let (_, router, _guard) = test_router().await?;

        let (status, body) = send(
            router,
            "GET",
            "/definitely-unmapped",
            Some(MANAGEMENT_TOKEN),
            Body::empty(),
        )
        .await?;

        assert_eq!(status, StatusCode::NOT_FOUND);

        let rendered = body_json(&body);

        assert_eq!(error_field(&rendered, "code"), "not_found");

        assert!(
            error_field(&rendered, "message")
                .as_str()
                .is_some_and(|message| message.contains("Invalid URL (GET /definitely-unmapped)")),
            "the message must name the method and path: {rendered}"
        );

        Ok(())
    }

    /// Verifies `start-provider-oauth` sweeps expired `oauth_states` rows
    /// before persisting its new state and answers the
    /// `{authorize_url, state, expires_at}` flow shape (README §3, §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database or the
    /// router.
    #[tokio::test]
    async fn start_provider_oauth_sweeps_expired_states_and_shapes_response()
    -> Result<(), Box<dyn StdError>> {
        let (database, router, _guard) = test_router().await?;

        let now = crate::unix_time::unix_now();

        storage::insert_oauth_state(
            &database,
            ProviderType::OpenAiOauth,
            "expired-state",
            "",
            now - 1,
        )
        .await?;

        let (status, body) = send(
            router,
            "POST",
            "/start-provider-oauth",
            Some(MANAGEMENT_TOKEN),
            test_support::body(r#"{"type":"openai_oauth"}"#).into(),
        )
        .await?;

        assert_eq!(status, StatusCode::OK);

        let flow = body_json(&body);

        let authorize_url = flow
            .get("authorize_url")
            .and_then(Value::as_str)
            .ok_or("the flow must carry authorize_url")?;

        assert!(
            authorize_url.starts_with("https://auth.openai.com/oauth/authorize"),
            "the openai_oauth flow must target the OpenAI authorize endpoint: {authorize_url}"
        );

        let state = flow
            .get("state")
            .and_then(Value::as_str)
            .ok_or("the flow must carry state")?;

        assert!(!state.is_empty(), "the state token must be non-empty");

        let expires_at = flow
            .get("expires_at")
            .and_then(Value::as_i64)
            .ok_or("the flow must carry expires_at")?;

        assert!(expires_at > now, "a fresh flow must not already be expired");

        // The expired row was swept; the freshly persisted state is the only
        // flow left in the table.
        let rows = database.query("SELECT state FROM oauth_states", ()).await?;

        let states: Vec<String> = rows
            .iter()
            .map(|row| row.get(0))
            .collect::<Result<_, _>>()?;

        assert_eq!(states, vec![state.to_string()]);

        Ok(())
    }

    /// Verifies the management body cap rejects oversized request bodies
    /// with HTTP 413 (README §4). The outward 64 MiB cap is deliberately
    /// not exercised by a test: materializing a 64 MiB oneshot body would
    /// dominate the test suite for a bound already pinned by the same
    /// axum layer.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the router.
    #[tokio::test]
    async fn rejects_oversized_management_bodies_with_413() -> Result<(), Box<dyn StdError>> {
        let (_, router, _guard) = test_router().await?;

        // One byte beyond the 2 MiB management cap (the router's
        // `MAX_BODY_BYTES`).
        let oversized = vec![b'a'; 2 * 1024 * 1024 + 1];

        let (status, _) = send(
            router,
            "POST",
            "/save-provider",
            Some(MANAGEMENT_TOKEN),
            Body::from(oversized),
        )
        .await?;

        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

        Ok(())
    }
}
