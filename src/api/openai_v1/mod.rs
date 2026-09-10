//! Outward OpenAI-compatible format sub-router (`/api/openai-v1`).

use super::json_response;
use crate::{
    database::Database,
    error::{ApiError, log_api_error},
    format::{ClientFormat, ClientSseRenderer, openai_chat_completion, openai_response},
    gateway::{self, IrEventStream},
    ir::{Aggregator, IrAggregation},
    provider::storage,
    security::constant_time_eq,
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, OriginalUri, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use log::debug;
use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

/// Maximum accepted request body size, large enough for inline image data.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Capacity of the channel between the SSE rendering task and the response
/// body stream.
///
/// Mirrors `gateway`'s `EVENT_CHANNEL_BUFFER` for the adjacent pipeline
/// stage: both hops buffer equally, so one slow stage cannot accumulate an
/// asymmetric backlog in the other.
const SSE_CHANNEL_BUFFER: usize = 64;

/// Constructs the format sub-router with OpenAI bearer authentication and its
/// own 404 fallback.
pub(crate) fn router() -> Router<Database> {
    Router::new()
        .route("/chat/completions", post(chat_completions))
        .route("/responses", post(responses))
        .route("/models", get(models))
        .fallback(fallback)
        .layer(middleware::from_fn(authenticate))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

/// Authenticates the request against the shared gateway API key using the
/// OpenAI `Authorization: Bearer <key>` convention.
async fn authenticate(request: Request, next: Next) -> Response {
    let is_authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(bearer_token)
        .is_some_and(|key| constant_time_eq(key, &crate::CONFIG.api.key));

    if !is_authorized {
        debug!("rejected an unauthorized API request");

        return json_response(401, ApiError::invalid_api_key().to_openai_json());
    }

    next.run(request).await
}

/// Extracts the bearer token from an `Authorization` header value.
///
/// The scheme is compared case-insensitively per RFC 9110 §11.1; the token
/// is taken verbatim after the first space, so malformed forms — no space,
/// tab separator, glued scheme, leading whitespace, trailing content — are
/// rejected by the scheme mismatch or by the credential comparison.
fn bearer_token(authorization: &str) -> Option<&str> {
    let (scheme, token) = authorization.split_once(' ')?;

    if scheme.eq_ignore_ascii_case("Bearer") {
        Some(token)
    } else {
        None
    }
}

/// Handles unmapped format paths with an OpenAI-shaped 404.
async fn fallback(OriginalUri(uri): OriginalUri, request: Request) -> Response {
    let mut error =
        ApiError::invalid_request(format!("Invalid URL ({} {})", request.method(), uri.path()));

    error.status = StatusCode::NOT_FOUND.as_u16();

    json_response(error.status, error.to_openai_json())
}

/// Handles `POST /chat/completions`.
async fn chat_completions(State(database): State<Database>, body: Bytes) -> Response {
    outcome_response(handle_completion::<openai_chat_completion::Format>(&database, &body).await)
}

/// Handles `POST /responses`.
async fn responses(State(database): State<Database>, body: Bytes) -> Response {
    outcome_response(handle_completion::<openai_response::Format>(&database, &body).await)
}

/// Renders a handler outcome, mapping failures to OpenAI error responses.
fn outcome_response(outcome: Result<Response, ApiError>) -> Response {
    match outcome {
        Ok(response) => response,
        Err(error) => json_response(error.status, error.to_openai_json()),
    }
}

/// Routes one completion request through the gateway in the given client
/// format: parse to IR, resolve routing, execute upstream, and render the
/// event stream live or aggregated.
///
/// Every failure that reaches this boundary is logged exactly once here,
/// with the request context available at each stage: a parse rejection names
/// only the endpoint, a routing rejection names the requested model, and
/// everything past routing additionally names the resolved provider.
///
/// # Errors
///
/// Returns [`ApiError`] for request, routing, and upstream failures.
async fn handle_completion<TFormat>(database: &Database, body: &[u8]) -> Result<Response, ApiError>
where
    TFormat: ClientFormat,
{
    let parsed = match TFormat::parse_request(body) {
        Ok(parsed) => parsed,
        Err(error) => {
            log_api_error("completion request", &error);

            return Err(error);
        }
    };

    let routing = match gateway::resolve_routing(database, &parsed.ir.model).await {
        Ok(routing) => routing,
        Err(error) => {
            log_api_error(
                &format!("completion for model `{}`", parsed.ir.model),
                &error,
            );

            return Err(error);
        }
    };

    let operation = format!(
        "completion for model `{}` via provider `{}`",
        routing.model.name, routing.provider.name
    );

    let public_name = routing.model.name.clone();

    let is_streaming = parsed.is_streaming;

    let should_include_usage = parsed.should_include_usage;

    let outcome = async {
        let events =
            gateway::execute(database, crate::http(), routing, parsed.ir, is_streaming).await?;

        if is_streaming {
            Ok(sse_response(
                events,
                TFormat::sse_renderer(should_include_usage, public_name),
            ))
        } else {
            aggregated_response(events, |aggregation| {
                TFormat::render_json_response(&public_name, aggregation)
            })
            .await
        }
    }
    .await;

    if let Err(error) = &outcome {
        log_api_error(&operation, error);
    }

    outcome
}

/// Handles `GET /models`.
async fn models(State(database): State<Database>) -> Response {
    match storage::list_enabled_models_with_owner(&database).await {
        Ok(listed) => {
            let data = listed
                .iter()
                .map(|(public_name, provider_name)| {
                    serde_json::json!({
                        "id": public_name,
                        "object": "model",
                        "created": 0,
                        "owned_by": provider_name,
                    })
                })
                .collect::<Vec<_>>();

            json_response(200, serde_json::json!({"object": "list", "data": data}))
        }

        Err(error) => {
            log_api_error("model listing", &error);

            json_response(error.status, error.to_openai_json())
        }
    }
}

/// Wraps an IR event stream into a live `text/event-stream` response.
///
/// # Panics
///
/// Panics if the static response parts fail to build, which cannot occur.
fn sse_response(
    events: IrEventStream,
    mut renderer: Box<dyn ClientSseRenderer + Send>,
) -> Response {
    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(SSE_CHANNEL_BUFFER);

    tokio::spawn(async move {
        let mut events = events;

        while let Some(item) = events.next().await {
            match item {
                Ok(event) => {
                    for frame in renderer.render(&event) {
                        if sender.send(Ok(frame.into())).await.is_err() {
                            return;
                        }
                    }
                }

                Err(error) => {
                    let _ = sender.send(Ok(renderer.error_frame(&error).into())).await;

                    return;
                }
            }
        }
    });

    #[expect(clippy::expect_used, reason = "SSE responses have known-valid parts")]
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(ReceiverStream::new(receiver)))
        .expect("static response parts are always valid")
}

/// Folds an IR event stream into one aggregated JSON response.
///
/// # Errors
///
/// Returns [`ApiError`] when the stream carries a failure.
async fn aggregated_response(
    events: IrEventStream,
    render: impl Fn(&IrAggregation) -> serde_json::Value,
) -> Result<Response, ApiError> {
    let mut aggregator = Aggregator::default();

    let mut events = events;

    while let Some(item) = events.next().await {
        match item {
            Ok(event) => aggregator.ingest(&event),
            Err(error) => return Err(error),
        }
    }

    Ok(json_response(200, render(&aggregator.finish())))
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
    /// router with that database as state and the database's deletion guard,
    /// so tests can seed routing rows without leaking the temporary file.
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
    ) -> Result<(StatusCode, String), Box<dyn StdError>> {
        let mut builder = HttpRequest::builder().method(method).uri(uri);

        if let Some(authorization) = authorization {
            builder = builder.header("authorization", authorization);
        }

        #[expect(clippy::expect_used, reason = "a build failure fails the test")]
        let request = builder.body(Body::empty()).expect("request should build");

        // SAFETY: not applicable; the in-memory oneshot serve is infallible,
        // so the unreachable arm documents the invariant.
        let response = match router.oneshot(request).await {
            Ok(response) => response,
            Err(error) => unreachable!("in-memory oneshot cannot fail: {error}"),
        };

        let status = response.status();

        let body = axum::body::to_bytes(response.into_body(), 1 << 20).await?;

        Ok((status, String::from_utf8_lossy(&body).to_string()))
    }

    /// Verifies that the format sub-router authenticates every request with
    /// the shared API key, serves the models listing only when authorized, and
    /// reports each effectively enabled model's public name as `id` with its
    /// owning provider's name as `owned_by`, hiding rows disabled by override
    /// or by provider (README §6.5).
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
    async fn authenticates_and_lists_models() -> Result<(), Box<dyn StdError>> {
        let (database, router, _guard) = test_router().await?;

        let (status, _) = send(router.clone(), "GET", "/models", None).await?;

        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _) = send(router.clone(), "GET", "/models", Some("Bearer wrong")).await?;

        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, body) = send(
            router.clone(),
            "GET",
            "/models",
            Some("Bearer test-api-key"),
        )
        .await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, r#"{"data":[],"object":"list"}"#);

        test_support::seed_minimal_provider(&database, "alpha", "", "test-key").await?;

        test_support::seed_model(&database, "alpha", "u1", "a-m1").await?;

        test_support::seed_model(&database, "alpha", "u2", "a-m2").await?;

        test_support::seed_model(&database, "alpha", "u3", "a-off").await?;

        database
            .execute(
                "UPDATE models SET enabled_override = 0 WHERE upstream_name = 'u3'",
                (),
            )
            .await?;

        test_support::seed_minimal_provider(&database, "beta", "", "test-key").await?;

        test_support::seed_model(&database, "beta", "u1", "b-m1").await?;

        test_support::seed_minimal_provider(&database, "gamma", "", "test-key").await?;

        test_support::seed_model(&database, "gamma", "u1", "g-m1").await?;

        database
            .execute("UPDATE providers SET enabled = 0 WHERE name = 'gamma'", ())
            .await?;

        let (status, body) = send(router, "GET", "/models", Some("Bearer test-api-key")).await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            concat!(
                r#"{"data":["#,
                r#"{"created":0,"id":"a-m1","object":"model","owned_by":"alpha"},"#,
                r#"{"created":0,"id":"a-m2","object":"model","owned_by":"alpha"},"#,
                r#"{"created":0,"id":"b-m1","object":"model","owned_by":"beta"}],"#,
                r#""object":"list"}"#
            )
        );

        Ok(())
    }

    /// Verifies that the authorization scheme is compared case-insensitively
    /// (RFC 9110 §11.1) while the credential stays byte-exact: lowercase and
    /// uppercase `bearer` schemes authorize with the exact key, and malformed
    /// forms — trailing content, tab separator, glued scheme, leading
    /// whitespace — remain rejected.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the router.
    #[tokio::test]
    async fn authenticates_bearer_scheme_case_insensitively() -> Result<(), Box<dyn StdError>> {
        let (_, router, _guard) = test_router().await?;

        for authorization in ["bearer test-api-key", "BEARER test-api-key"] {
            let (status, _) = send(router.clone(), "GET", "/models", Some(authorization)).await?;

            assert_eq!(status, StatusCode::OK, "{authorization:?} should authorize");
        }

        for authorization in [
            "bearer test-api-key extra",
            "bearer\ttest-api-key",
            "bearerkey test-api-key",
            " bearer test-api-key",
        ] {
            let (status, _) = send(router.clone(), "GET", "/models", Some(authorization)).await?;

            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{authorization:?} must stay rejected"
            );
        }

        Ok(())
    }

    /// Verifies that unmapped format paths yield the OpenAI-shaped 404 with
    /// the original request path.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the router.
    #[tokio::test]
    async fn fallback_returns_openai_404_shape() -> Result<(), Box<dyn StdError>> {
        let (_, router, _guard) = test_router().await?;

        let (status, body) = send(router, "GET", "/bogus", Some("Bearer test-api-key")).await?;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("Invalid URL (GET "));
        assert!(body.contains("/bogus)"));
        assert!(body.contains("invalid_request_error"));

        Ok(())
    }
}
