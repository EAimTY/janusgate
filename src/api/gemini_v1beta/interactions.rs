//! Gemini Interactions endpoint: `POST /interactions`.
//!
//! The Interactions surface addresses its model in the request body (unlike GenerateContent,
//! whose model rides the URL path), so parsing takes the public model id from the body; only the
//! streaming mode may arrive from the route — the `?alt=sse` query forces streaming, otherwise
//! the body's `stream` member decides. Request parsing and rendering go through
//! [`crate::format::gemini_interaction::inbound`]; errors render through the prefix
//! [`super::error_response`] (the Gemini envelope); accounting records the endpoint as
//! [`ApiLogEndpointType::GeminiInteractions`](crate::api_logs::ApiLogEndpointType).
//!
//! Shared prefix concerns (authentication, 404/405 bodies, body limit, JSON helper, accounting
//! row id) live in [`super`]; this module owns only the endpoint handler and its pipeline.

use super::AuthenticatedApiKeyRow;
use crate::{
    api::{Error, log_error},
    api_logs::{self, ApiLogDraft, ApiLogOutcome},
    context::Context,
    format::{
        ClientFormat, ClientSseRenderer, RouteContext,
        gemini_interaction::inbound::Format as InteractionsFormat,
    },
    gateway::{self, IrEventStream},
    ir::{Aggregator, IrAggregation, IrEvent, IrUsage},
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{OriginalUri, State, rejection::BytesRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use axum_extra::extract::WithRejection;
use serde::Serialize;
use std::io::Error as IoError;
use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

/// Capacity of the channel between the SSE rendering task and the response body stream.
///
/// Mirrors `gateway`'s event buffer for the adjacent pipeline stage: both hops buffer equally, so
/// one slow stage cannot accumulate an asymmetric backlog in the other.
const SSE_CHANNEL_BUFFER: usize = 64;

/// Builds the endpoint routes; the prefix router mounts them.
pub(super) fn router() -> Router<Context> {
    Router::new().route("/interactions", post(interactions))
}

/// Handles `POST /interactions`.
async fn interactions(
    State(context): State<Context>,
    api_key: AuthenticatedApiKeyRow,
    OriginalUri(uri): OriginalUri,
    WithRejection(body, _): WithRejection<Bytes, BodyError>,
) -> Response {
    outcome_response(handle_completion(&context, &body, api_key.0, is_alt_sse(&uri)).await)
}

/// Reads the `?alt=sse` streaming override of the API's own `stream` + `alt=sse` convention
/// straight off the request URI: axum's `Query` extractor is not compiled in, and the one pair
/// this surface reads needs no form decoding — the override is exactly `alt=sse`.
fn is_alt_sse(uri: &axum::http::Uri) -> bool {
    uri.query().is_some_and(|query| {
        query
            .split('&')
            .any(|pair| matches!(pair.split_once('='), Some(("alt", "sse"))))
    })
}

/// Renders a handler outcome, mapping failures to Gemini error responses.
fn outcome_response(outcome: Result<Response, Error>) -> Response {
    match outcome {
        Ok(response) => response,
        Err(error) => super::error_response(error),
    }
}

/// Body extractor rejection mapped into the surface's Gemini error shape: an over-limit body
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

        super::error_response(self.0)
    }
}

/// Routes one Interactions request through the gateway: parse to IR with the route's streaming
/// override, resolve routing from the body-routed model, execute upstream, and render the event
/// stream live or aggregated.
///
/// Every failure that reaches this boundary is logged exactly once here, with the request context
/// available at each stage: a parse rejection names only the endpoint, a routing rejection names
/// the requested model, and everything past routing additionally names the resolved provider.
///
/// `api_key_id` — extracted from the authentication middleware's stamp — anchors the request's
/// accounting draft, which this function builds from the routing facts and hands to
/// [`gateway::execute`] for the dispatch mark. Exactly one of the three finalization points then
/// records the row: [`gateway::execute`] itself for a post-dispatch failure, the aggregation path
/// below for a non-streaming response, or the SSE render task for a streaming one.
///
/// # Errors
///
/// Returns [`Error`] for request, routing, and upstream failures.
async fn handle_completion(
    context: &Context,
    body: &[u8],
    api_key_id: i64,
    is_alt_sse: bool,
) -> Result<Response, Error> {
    // The `?alt=sse` query forces streaming; otherwise the body's `stream` member decides. The
    // model always rides the body on this surface, so the context carries no public id.
    let route = RouteContext {
        public_id: None,
        is_streaming: is_alt_sse.then_some(true),
    };

    let parsed = match InteractionsFormat::parse_request(body, &route) {
        Ok(parsed) => parsed,

        Err(error) => {
            let error = Error::from(error);

            log_error("interaction request", &error);

            return Err(error);
        }
    };

    let routing = match gateway::resolve_routing(&context.database, &parsed.ir.model).await {
        Ok(routing) => routing,

        Err(error) => {
            let error = Error::from(error);

            log_error(
                &format!("interaction for model `{}`", parsed.ir.model),
                &error,
            );

            return Err(error);
        }
    };

    let operation = format!(
        "interaction for model `{}` via provider `{}`",
        routing.model.public_id, routing.provider.name
    );

    let public_id = routing.model.public_id.clone();

    let is_streaming = parsed.is_streaming;

    let should_include_usage = parsed.should_include_usage;

    // The accounting draft: every fact known before dispatch, built from the authenticated key
    // and the routing row; `gateway::execute` stamps the dispatch instant into it.
    let mut log = ApiLogDraft {
        started_at: None,
        api_key_id,
        provider_id: routing.provider.id,
        model_id: routing.model.id,
        endpoint_type: InteractionsFormat::ENDPOINT_TYPE,
        is_stream: is_streaming,
    };

    let outcome: Result<Response, Error> = async {
        let events = gateway::execute(context, routing, parsed.ir, is_streaming, &mut log).await?;

        if is_streaming {
            Ok(sse_response(
                events,
                InteractionsFormat::sse_renderer(should_include_usage, public_id),
                context.database.clone(),
                log,
            ))
        } else {
            // Finalization point 2: the non-streaming path records the row synchronously after
            // aggregation and body rendering, before the Response leaves — `completed` + 200 on
            // success, the classified outcome and status on a stream failure.
            match aggregated_response(events, |aggregation| {
                InteractionsFormat::render_json_response(&public_id, aggregation)
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
                    let error = Error::from(error);
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

/// Wraps an IR event stream into a live `text/event-stream` response, carrying the request's
/// accounting draft and database handle into the render task: that task is the streaming
/// finalization point and records exactly one row when the stream closes — `completed` + 200 for
/// a clean end, the classified outcome under the already-declared 200 for an error frame (the
/// first enqueued frame declares the status, so an error frame never changes it), and
/// `client_aborted` for a broken body channel — with 200 only once a frame had been enqueued and
/// `NULL` before the first. The recorded usage is whatever the stream delivered before closure.
///
/// # Panics
///
/// Panics if the static response parts fail to build, which cannot occur.
fn sse_response(
    events: IrEventStream,
    mut renderer: Box<dyn ClientSseRenderer + Send>,
    database: crate::database::Database,
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
                    // outcome moves to the classified failure.
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
/// before the first, when no response ever reached it.
fn declared_status(is_status_declared: bool) -> Option<i64> {
    if is_status_declared {
        Some(i64::from(StatusCode::OK.as_u16()))
    } else {
        None
    }
}

/// Folds an IR event stream into one aggregated JSON response, yielding the response beside the
/// terminal usage the aggregation observed — `None` when the stream carried no usage report, and
/// always `None` for a failed stream, whose error carries no terminal usage. Failures stay in the
/// gateway domain; the caller classifies them once at its boundary.
///
/// # Errors
///
/// Returns the stream's failure when it carries one.
async fn aggregated_response<TBody>(
    events: IrEventStream,
    render_json_fn: impl Fn(&IrAggregation) -> TBody,
) -> Result<(Response, Option<IrUsage>), gateway::Error>
where
    TBody: Serialize,
{
    let mut aggregator = Aggregator::default();

    let mut events = events;

    while let Some(item) = events.next().await {
        match item {
            Ok(event) => aggregator.ingest(&event),
            Err(error) => return Err(error),
        }
    }

    let aggregation = aggregator.finish();
    let usage = aggregation.usage;

    Ok((
        super::json_response(StatusCode::OK, &render_json_fn(&aggregation)),
        usage,
    ))
}
