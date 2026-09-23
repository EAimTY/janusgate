//! Anthropic inbound surface (`/api/anthropic-v1`).
//!
//! `POST /messages` (stream via body `stream: true`, `max_tokens` required per spec) and
//! `GET /models` per REFACTOR §§4–5. Authentication is `x-api-key` only with the
//! `anthropic-version` header required present (value not validated). Request parsing and
//! rendering go through [`crate::format::anthropic_messages`]; errors render through
//! [`crate::format::error_anthropic`]; the model stays in the body for this surface, so parsing
//! uses [`crate::format::RouteContext::empty`]; the accounting endpoint is
//! [`crate::api_logs::ApiLogEndpointType::AnthropicMessages`].

use crate::{
    api::{Error, log_error},
    api_keys,
    api_logs::{self, ApiLogDraft, ApiLogOutcome},
    context::Context,
    database::Database,
    format::{
        ClientFormat, ClientSseRenderer, RouteContext,
        anthropic_messages::Format as MessagesFormat, error_anthropic,
    },
    gateway::{self, IrEventStream},
    ir::{Aggregator, IrAggregation, IrEvent, IrUsage},
    provider::storage,
    security,
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, FromRequestParts, Request, State, rejection::BytesRejection},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::extract::WithRejection;
use serde::Serialize;
use std::{convert::Infallible, io::Error as IoError};
use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

/// Maximum accepted request body size, matching the OpenAI surfaces.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Capacity of the channel between the SSE rendering task and the response body stream.
///
/// Mirrors `gateway`'s `EVENT_CHANNEL_BUFFER` for the adjacent pipeline stage: both hops buffer
/// equally, so one slow stage cannot accumulate an asymmetric backlog in the other.
const SSE_CHANNEL_BUFFER: usize = 64;

/// The `GET /models` success body in the native Anthropic shape: the full effective-model set in
/// one page (`has_more: false`), ignoring pagination parameters.
#[derive(Debug, Serialize)]
struct ModelList<'a> {
    /// One entry per served model, ordered by public ID.
    data: Vec<ModelObject<'a>>,

    /// Always `false`: the gateway returns the full list in one page.
    has_more: bool,
}

/// One served model in the native Anthropic model shape.
#[derive(Debug, Serialize)]
struct ModelObject<'a> {
    /// Always `model`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// Public routing ID (README §3).
    id: &'a str,
}

/// Constructs the surface router with Anthropic-key authentication and its own 404/405 responses.
pub(super) fn router(database: Database) -> Router<Context> {
    Router::new()
        .route("/messages", post(messages))
        .route("/models", get(models))
        .fallback(fallback)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn_with_state(database, authenticate))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

/// The row id of the gateway API key that authorized the current request.
#[derive(Debug, Clone, Copy)]
struct AuthenticatedApiKeyRow(i64);

impl<S> FromRequestParts<S> for AuthenticatedApiKeyRow
where
    S: Send + Sync,
{
    /// Never constructed: this extractor fails fast instead of rejecting.
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        #[expect(
            clippy::expect_used,
            reason = "the authentication middleware always stamps this extension before a \
                      completion handler runs"
        )]
        let row_id = parts
            .extensions
            .get::<AuthenticatedApiKeyRow>()
            .expect("an authenticated completion request carries its API key row id")
            .0;

        Ok(Self(row_id))
    }
}

/// Authenticates the request against the enabled gateway API keys using the Anthropic `x-api-key`
/// header convention, requiring the `anthropic-version` header present (value not validated).
async fn authenticate(State(database): State<Database>, request: Request, next: Next) -> Response {
    let has_version = request.headers().contains_key("anthropic-version");

    let presented = request
        .headers()
        .get("x-api-key")
        .and_then(|value| value.to_str().ok());

    let Some(presented) = presented else {
        log_error("request", &Error::InvalidApiKey);

        return error_response(Error::InvalidApiKey);
    };

    if !has_version {
        let error = Error::MalformedRequest {
            param: Some("anthropic-version".to_string()),
            message: "The anthropic-version header is required".to_string(),
        };

        log_error("request", &error);

        return error_response(error);
    }

    match api_keys::list_enabled_api_key_secrets(&database).await {
        Ok(keys) => {
            let matched = keys
                .into_iter()
                .find(|(_, secret)| security::constant_time_eq(presented, secret));

            match matched {
                Some((row_id, _secret)) => {
                    let mut request = request;

                    request
                        .extensions_mut()
                        .insert(AuthenticatedApiKeyRow(row_id));

                    next.run(request).await
                }

                None => {
                    log_error("request", &Error::InvalidApiKey);

                    error_response(Error::InvalidApiKey)
                }
            }
        }

        Err(error) => {
            let error = Error::from(error);

            log_error("request", &error);

            error_response(error)
        }
    }
}

/// Handles unmapped surface paths with an Anthropic-shaped 404.
async fn fallback(
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    request: Request,
) -> Response {
    let error = Error::InvalidUrl {
        method: request.method().as_str().to_string(),
        path: uri.path().to_string(),
    };

    log_error("request", &error);

    error_response(error)
}

/// Handles a request whose method is not registered on an otherwise-matching path with an
/// Anthropic-shaped 405.
async fn method_not_allowed(method: Method, uri: axum::extract::OriginalUri) -> Response {
    let error = Error::MethodNotAllowed;

    log_error(
        &format!("request {} {}", method.as_str(), uri.0.path()),
        &error,
    );

    error_response(error)
}

/// Renders one API failure as an Anthropic JSON error response through the format's render
/// module.
fn error_response(error: Error) -> Response {
    let (status, envelope) = error_anthropic::render(&error);

    json_response(status, &envelope)
}

/// Body extractor rejection mapped into the surface's Anthropic error shape: an over-limit body
/// renders the surface's 413, every other body failure renders as a malformed request.
#[derive(Debug)]
struct BodyError(Error);

impl From<BytesRejection> for BodyError {
    fn from(rejection: BytesRejection) -> Self {
        Self(Error::from(rejection))
    }
}

impl IntoResponse for BodyError {
    fn into_response(self) -> Response {
        log_error("request body", &self.0);

        error_response(self.0)
    }
}

/// Handles `POST /messages`.
async fn messages(
    State(context): State<Context>,
    api_key: AuthenticatedApiKeyRow,
    WithRejection(body, _): WithRejection<Bytes, BodyError>,
) -> Response {
    outcome_response(handle_completion::<MessagesFormat>(&context, &body, api_key.0).await)
}

/// Renders a handler outcome, mapping failures to Anthropic error responses.
fn outcome_response(outcome: Result<Response, Error>) -> Response {
    match outcome {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

/// Routes one completion request through the gateway in the Messages client format: parse to IR,
/// resolve routing, execute upstream, and render the event stream live or aggregated.
///
/// Every failure that reaches this boundary is logged exactly once here, with the request context
/// available at each stage: a parse rejection names only the endpoint, a routing rejection names
/// the requested model, and everything past routing additionally names the resolved provider.
///
/// `api_key_id` — extracted from the authentication middleware's stamp — anchors the request's
/// accounting draft, which this function builds from the routing facts and hands to
/// [`gateway::execute`] for the dispatch mark. Exactly one of the three finalization points then
/// records the row: [`gateway::execute`] itself for a post-dispatch failure, the aggregation path
/// below for a non-streaming response, or the SSE render task for a streaming one (README §3
/// *ApiLog*).
///
/// # Errors
///
/// Returns [`Error`] for request, routing, and upstream failures.
async fn handle_completion<TFormat>(
    context: &Context,
    body: &[u8],
    api_key_id: i64,
) -> Result<Response, Error>
where
    TFormat: ClientFormat,
    TFormat::JsonResponse: Serialize,
{
    // The model stays in the body for this surface: no route context overrides it.
    let parsed = match TFormat::parse_request(body, &RouteContext::empty()) {
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
        routing.model.public_id, routing.provider.name
    );

    let public_id = routing.model.public_id.clone();

    let is_streaming = parsed.is_streaming;

    let should_include_usage = parsed.should_include_usage;

    // The accounting draft: every fact known before dispatch, built from the authenticated key
    // and the routing row; `gateway::execute` stamps the dispatch instant into it (README §3
    // *ApiLog*).
    let mut log = ApiLogDraft {
        started_at: None,
        api_key_id,
        provider_id: routing.provider.id,
        model_id: routing.model.id,
        endpoint_type: TFormat::ENDPOINT_TYPE,
        is_stream: is_streaming,
    };

    let outcome = async {
        let events = gateway::execute(context, routing, parsed.ir, is_streaming, &mut log).await?;

        if is_streaming {
            Ok(sse_response(
                events,
                TFormat::sse_renderer(should_include_usage, public_id),
                context.database.clone(),
                log,
            ))
        } else {
            // Finalization point 2: the non-streaming path records the row synchronously after
            // aggregation and body rendering, before the Response leaves — `completed` + 200 on
            // success, the classified outcome and status on a stream failure (README §3 *ApiLog*).
            match aggregated_response(events, |aggregation| {
                TFormat::render_json_response(&public_id, aggregation)
            })
            .await
            {
                Ok((response, usage)) => {
                    api_logs::finalize_api_log(
                        &context.database,
                        &log,
                        ApiLogOutcome::Completed,
                        Some(i64::from(StatusCode::OK.as_u16())),
                        usage,
                    )
                    .await;

                    Ok(response)
                }

                Err(error) => {
                    let (outcome, http_status) = error.accounting();

                    api_logs::finalize_api_log(
                        &context.database,
                        &log,
                        outcome,
                        Some(http_status),
                        None,
                    )
                    .await;

                    Err(error)
                }
            }
        }
    }
    .await;

    if let Err(error) = &outcome {
        log_error(&operation, error);
    }

    outcome
}

/// Handles `GET /models` in the native Anthropic shape over the same effective-model set every
/// surface lists, ignoring pagination parameters and returning the full list in one page.
async fn models(State(context): State<Context>) -> Response {
    match storage::list_enabled_models_with_owner(&context.database).await {
        Ok(listed) => {
            let data = listed
                .iter()
                .map(|(public_id, _provider_name)| ModelObject {
                    kind: "model",
                    id: public_id,
                })
                .collect();

            json_response(
                StatusCode::OK,
                &ModelList {
                    data,
                    has_more: false,
                },
            )
        }

        Err(error) => {
            let error = Error::from(error);

            log_error("model listing", &error);

            error_response(error)
        }
    }
}

/// Wraps an IR event stream into a live `text/event-stream` response, carrying the request's
/// accounting draft and database handle into the render task: that task is the streaming
/// finalization point (README §3 *ApiLog*) and records exactly one row when the stream closes —
/// `completed` + 200 for a clean end, the classified outcome under the already-declared 200 for
/// an error frame (the first enqueued frame declares the status, so an error frame never changes
/// it), and `client_aborted` for a broken body channel — with 200 only once a frame had been
/// enqueued and `NULL` before the first. The recorded usage is whatever the stream delivered
/// before closure.
///
/// # Panics
///
/// Panics if the static response parts fail to build, which cannot occur.
fn sse_response(
    events: IrEventStream,
    mut renderer: Box<dyn ClientSseRenderer + Send>,
    database: Database,
    log: ApiLogDraft,
) -> Response {
    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<Bytes, IoError>>(SSE_CHANNEL_BUFFER);

    tokio::spawn(async move {
        let mut events = events;

        // Whether any frame reached the client's channel; the first one declares HTTP 200.
        let mut is_status_declared = false;

        // The last usage the stream reported, recorded best-effort at finalization.
        let mut usage: Option<IrUsage> = None;

        while let Some(item) = events.next().await {
            match item {
                Ok(event) => {
                    if let IrEvent::Usage(reported) = &event {
                        usage = Some(*reported);
                    }

                    for frame in renderer.render(&event) {
                        if sender.send(Ok(frame.into())).await.is_err() {
                            // The client's body channel is gone: a client abort, keyed only on
                            // what was actually enqueued before it broke.
                            api_logs::finalize_api_log(
                                &database,
                                &log,
                                ApiLogOutcome::ClientAborted,
                                declared_status(is_status_declared),
                                usage,
                            )
                            .await;

                            return;
                        }

                        is_status_declared = true;
                    }
                }

                Err(gateway_error) => {
                    let error = Error::from(gateway_error);
                    let (outcome, _) = error.accounting();

                    if sender
                        .send(Ok(renderer.error_frame(&error).into()))
                        .await
                        .is_err()
                    {
                        // The channel broke before the error frame could be enqueued: the client
                        // left, so the disconnect rules decide alone.
                        api_logs::finalize_api_log(
                            &database,
                            &log,
                            ApiLogOutcome::ClientAborted,
                            declared_status(is_status_declared),
                            usage,
                        )
                        .await;

                        return;
                    }

                    // The error frame never changes the status already declared; only the
                    // outcome moves to the classified failure (README §3 *ApiLog*).
                    api_logs::finalize_api_log(
                        &database,
                        &log,
                        outcome,
                        Some(i64::from(StatusCode::OK.as_u16())),
                        usage,
                    )
                    .await;

                    return;
                }
            }
        }

        // A clean end of stream: every event rendered and the body closed normally.
        api_logs::finalize_api_log(
            &database,
            &log,
            ApiLogOutcome::Completed,
            Some(i64::from(StatusCode::OK.as_u16())),
            usage,
        )
        .await;
    });

    #[expect(clippy::expect_used, reason = "SSE responses have known-valid parts")]
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(ReceiverStream::new(receiver)))
        .expect("static response parts are always valid")
}

/// The status recorded for a stream close: 200 once any frame — an error frame included — was
/// enqueued, because the first enqueued frame is what declares the response to the client; `NULL`
/// before the first, when no response ever reached it (README §3 *ApiLog*).
fn declared_status(is_status_declared: bool) -> Option<i64> {
    if is_status_declared {
        Some(i64::from(StatusCode::OK.as_u16()))
    } else {
        None
    }
}

/// Folds an IR event stream into one aggregated JSON response, yielding the response beside the
/// terminal usage the aggregation observed — `None` when the stream carried no usage report, and
/// always `None` for a failed stream, whose error carries no terminal usage.
///
/// # Errors
///
/// Returns the stream's failure when it carries one.
async fn aggregated_response<TBody>(
    events: IrEventStream,
    render_json_fn: impl Fn(&IrAggregation) -> TBody,
) -> Result<(Response, Option<IrUsage>), Error>
where
    TBody: Serialize,
{
    let mut aggregator = Aggregator::default();

    let mut events = events;

    while let Some(item) = events.next().await {
        match item {
            Ok(event) => aggregator.ingest(&event),
            Err(error) => return Err(Error::from(error)),
        }
    }

    let aggregation = aggregator.finish();
    let usage = aggregation.usage;

    Ok((
        json_response(StatusCode::OK, &render_json_fn(&aggregation)),
        usage,
    ))
}

/// Builds a JSON response with the given status code from a typed payload.
///
/// # Panics
///
/// Panics if the rendered body fails to serialize or the static response parts fail to build,
/// neither of which can occur.
fn json_response<T>(status: StatusCode, body: &T) -> Response
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
