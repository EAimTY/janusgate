//! Outward OpenAI-compatible format sub-router (`/api/openai-v1`).

mod error;

use crate::{
    context::Context,
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
use error::{Error, json_response, log_error};
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
pub(crate) fn router() -> Router<Context> {
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

        return Error::InvalidApiKey.into_response();
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
    Error::InvalidUrl {
        method: request.method().as_str().to_string(),
        path: uri.path().to_string(),
    }
    .into_response()
}

/// Handles `POST /chat/completions`.
async fn chat_completions(State(context): State<Context>, body: Bytes) -> Response {
    outcome_response(handle_completion::<openai_chat_completion::Format>(&context, &body).await)
}

/// Handles `POST /responses`.
async fn responses(State(context): State<Context>, body: Bytes) -> Response {
    outcome_response(handle_completion::<openai_response::Format>(&context, &body).await)
}

/// Renders a handler outcome, mapping failures to OpenAI error responses.
fn outcome_response(outcome: Result<Response, Error>) -> Response {
    match outcome {
        Ok(response) => response,
        Err(error) => error.into_response(),
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
/// Returns [`Error`] for request, routing, and upstream failures.
async fn handle_completion<TFormat>(context: &Context, body: &[u8]) -> Result<Response, Error>
where
    TFormat: ClientFormat,
{
    let parsed = match TFormat::parse_request(body) {
        Ok(parsed) => parsed,
        Err(error) => {
            let error = Error::from(error);

            log_error("completion request", &error);

            return Err(error);
        }
    };

    let routing = match gateway::resolve_routing(&context.database, &parsed.ir.model).await {
        Ok(routing) => routing,
        Err(error) => {
            let error = Error::from(error);

            log_error(
                &format!("completion for model `{}`", parsed.ir.model),
                &error,
            );

            return Err(error);
        }
    };

    let operation = format!(
        "completion for model `{}` via provider `{}`",
        routing.model.id, routing.provider.name
    );

    let public_id = routing.model.id.clone();

    let is_streaming = parsed.is_streaming;

    let should_include_usage = parsed.should_include_usage;

    let outcome = async {
        let events = gateway::execute(context, routing, parsed.ir, is_streaming).await?;

        if is_streaming {
            Ok(sse_response(
                events,
                TFormat::sse_renderer(should_include_usage, public_id),
            ))
        } else {
            aggregated_response(events, |aggregation| {
                TFormat::render_json_response(&public_id, aggregation)
            })
            .await
        }
    }
    .await;

    let outcome = outcome.map_err(Error::from);

    if let Err(error) = &outcome {
        log_error(&operation, error);
    }

    outcome
}

/// Handles `GET /models`.
async fn models(State(context): State<Context>) -> Response {
    match storage::list_enabled_models_with_owner(&context.database).await {
        Ok(listed) => {
            let data = listed
                .iter()
                .map(|(public_id, provider_name)| {
                    serde_json::json!({
                        "id": public_id,
                        "object": "model",
                        "created": 0,
                        "owned_by": provider_name,
                    })
                })
                .collect::<Vec<_>>();

            json_response(200, serde_json::json!({"object": "list", "data": data}))
        }

        Err(error) => {
            let error = Error::from(error);

            log_error("model listing", &error);

            error.into_response()
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

                Err(gateway_error) => {
                    let payload = Error::from(gateway_error).to_openai_json();

                    let _ = sender.send(Ok(renderer.error_frame(&payload).into())).await;

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
/// Returns the stream's failure when it carries one.
async fn aggregated_response(
    events: IrEventStream,
    render: impl Fn(&IrAggregation) -> serde_json::Value,
) -> Result<Response, gateway::Error> {
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
