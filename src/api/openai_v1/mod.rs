//! Outward OpenAI-compatible format sub-router (`/api/openai-v1`).

use super::json_response;
use crate::{
    database::Database,
    error::ApiError,
    format::{ClientFormat, ClientSseRenderer, openai_chat_completion, openai_response},
    gateway::{self, IrEventStream},
    ir::{Aggregator, IrAggregation},
    provider::storage,
    security::constant_time_eq,
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{OriginalUri, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

/// Maximum accepted request body size, large enough for inline image data.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Constructs the format sub-router with OpenAI bearer authentication and its
/// own 404 fallback.
pub(crate) fn router() -> Router<Database> {
    Router::new()
        .route("/chat/completions", post(chat_completions))
        .route("/responses", post(responses))
        .route("/models", get(models))
        .fallback(fallback)
        .layer(middleware::from_fn(authenticate))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
}

/// Authenticates the request against the shared gateway API key using the
/// OpenAI `Authorization: Bearer <key>` convention.
async fn authenticate(request: Request, next: Next) -> Response {
    let is_authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|key| constant_time_eq(key, &crate::CONFIG.api.key));

    if !is_authorized {
        return json_response(401, ApiError::invalid_api_key().to_openai_json());
    }

    next.run(request).await
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
/// # Errors
///
/// Returns [`ApiError`] for request, routing, and upstream failures.
async fn handle_completion<TFormat>(database: &Database, body: &[u8]) -> Result<Response, ApiError>
where
    TFormat: ClientFormat,
{
    let parsed = TFormat::parse_request(body)?;

    let routing = gateway::resolve_routing(database, &parsed.ir.model).await?;

    let public_name = routing.model.name.clone();

    let is_streaming = parsed.stream;

    let events =
        gateway::execute(database, crate::http(), routing, parsed.ir, is_streaming).await?;

    if is_streaming {
        Ok(sse_response(
            events,
            TFormat::sse_renderer(parsed.include_usage, public_name),
        ))
    } else {
        aggregated_response(events, |aggregation| {
            TFormat::render_json_response(&public_name, aggregation)
        })
        .await
    }
}

/// Handles `GET /models`.
async fn models(State(database): State<Database>) -> Response {
    match storage::list_enabled_public_names(&database).await {
        Ok(names) => {
            let data = names
                .iter()
                .map(|name| {
                    serde_json::json!({
                        "id": name,
                        "object": "model",
                        "created": 0,
                        "owned_by": "janusgate",
                    })
                })
                .collect::<Vec<_>>();

            json_response(200, serde_json::json!({"object": "list", "data": data}))
        }

        Err(error) => json_response(error.status, error.to_openai_json()),
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
        tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(64);

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
    use crate::database::Database;
    use axum::{
        body::Body,
        http::{Request as HttpRequest, StatusCode},
    };
    use std::sync::Once;
    use tower::ServiceExt as _;

    /// Writes a test configuration file and initializes the global
    /// configuration exactly once per process.
    ///
    /// # Panics
    ///
    /// Panics if the configuration file cannot be written.
    fn init_config() {
        static INIT: Once = Once::new();

        INIT.call_once(|| {
            let path = std::env::temp_dir().join("janusgate_openai_v1_test_config.jsonc");

            #[expect(
                clippy::expect_used,
                reason = "a configuration write failure fails the test"
            )]
            std::fs::write(
                &path,
                r#"{
                    "listen": "127.0.0.1:1",
                    "database": {"path": "irrelevant.db", "max_connections": 1, "pool_timeout": 1},
                    "management_api": {"token": "test-mgmt"},
                    "api": {"key": "test-api-key"}
                }"#,
            )
            .expect("config file should write");

            crate::tests::set_config_for_tests(crate::parse_config([
                "janusgate".to_string(),
                "-c".to_string(),
                path.to_string_lossy().to_string(),
            ]));
        });
    }

    /// Opens an isolated test database and returns the router with state.
    ///
    /// # Panics
    ///
    /// Panics if initialization fails.
    async fn test_router() -> axum::Router {
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        init_config();

        let unique_id = COUNTER.fetch_add(1, Ordering::Relaxed);

        let pool_size = {
            #[expect(clippy::expect_used, reason = "two is non-zero")]
            std::num::NonZero::new(2).expect("non-zero")
        };

        #[expect(clippy::expect_used, reason = "an open failure fails the test")]
        let database = Database::open(
            std::env::temp_dir().join(format!(
                "janusgate_openai_v1_{}_{}.db",
                std::process::id(),
                unique_id
            )),
            pool_size,
            std::time::Duration::from_secs(5),
        )
        .await
        .expect("database should open");

        router().with_state(database)
    }

    /// Sends a request through the router and returns status and body.
    ///
    /// # Panics
    ///
    /// Panics if the request cannot be built or served.
    async fn send(
        router: axum::Router,
        method: &str,
        uri: &str,
        authorization: Option<&str>,
    ) -> (StatusCode, String) {
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

        #[expect(clippy::expect_used, reason = "a body read failure fails the test")]
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body");

        (status, String::from_utf8_lossy(&body).to_string())
    }

    /// Verifies that the format sub-router authenticates every request with
    /// the shared API key and serves the models listing only when authorized.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn authenticates_and_lists_models() {
        let router = test_router().await;

        let (status, _) = send(router.clone(), "GET", "/models", None).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _) = send(router.clone(), "GET", "/models", Some("Bearer wrong")).await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, body) = send(router, "GET", "/models", Some("Bearer test-api-key")).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, r#"{"data":[],"object":"list"}"#);
    }

    /// Verifies that unmapped format paths yield the OpenAI-shaped 404 with
    /// the original request path.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn fallback_returns_openai_404_shape() {
        let router = test_router().await;

        let (status, body) = send(router, "GET", "/bogus", Some("Bearer test-api-key")).await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("Invalid URL (GET "));
        assert!(body.contains("/bogus)"));
        assert!(body.contains("invalid_request_error"));
    }
}
