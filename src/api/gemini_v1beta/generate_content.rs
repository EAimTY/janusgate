//! Gemini GenerateContent endpoints (`/api/gemini-v1beta/models/...`).
//!
//! Implements `GET /models`, `POST /models/{model}:generateContent` (non-streaming), and
//! `POST /models/{model}:streamGenerateContent?alt=sse` (streaming — only the `alt=sse` form
//! is supported, else 400) per REFACTOR §§4–5 and README §§6.4–6.6.
//!
//! Routing notes: axum 0.8 does not support partial-segment parameters such as
//! `/models/{model}:generateContent` — constructing such a route panics with "Only one
//! parameter is allowed per path segment" (verified with a build-time probe against axum
//! 0.8.9; see [`router`] tests). The endpoints therefore share one wildcard route,
//! `/models/{*rest}`, plus manual parsing in [`parse_model_action`]; malformed paths return
//! the same Gemini 404 the prefix fallback renders, so the observable behavior matches a
//! native partial-segment router.
//!
//! Shared prefix concerns (authentication, 404/405 bodies, body limit, JSON helper,
//! accounting row id) live in [`super`]; this module owns only the three endpoint handlers,
//! request parsing through [`crate::format::gemini_inbound`], and the native model-listing
//! shape.

use crate::{
    api::{Error, log_error},
    api_logs::{self, ApiLogDraft, ApiLogOutcome},
    context::Context,
    database::Database,
    format::{
        ClientFormat, ClientSseRenderer, RouteContext, gemini_inbound::Format as InboundFormat,
    },
    gateway::{self, IrEventStream},
    ir::{Aggregator, IrAggregation, IrEvent, IrUsage},
    provider::storage,
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{OriginalUri, State, rejection::BytesRejection},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::extract::WithRejection;
use serde::Serialize;
use std::io::Error as IoError;
use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

/// Capacity of the channel between the SSE rendering task and the response body stream.
///
/// Mirrors `gateway`'s event buffer and the OpenAI surface's channel buffer: both hops buffer
/// equally, so one slow stage cannot accumulate an asymmetric backlog in the other.
const SSE_CHANNEL_BUFFER: usize = 64;

/// Builds the endpoint routes; the prefix router mounts them.
pub(super) fn router() -> Router<Context> {
    Router::new()
        .route("/models", get(models))
        .route("/models/{*rest}", post(dispatch))
}

/// The `GET /models` success body: the native Gemini list shape over the effective-model
/// set, with no pagination token.
#[derive(Debug, Serialize)]
struct ModelList {
    /// One entry per served model, ordered by public ID.
    models: Vec<ModelEntry>,
}

/// One served model in the native Gemini shape.
#[derive(Debug, Serialize)]
struct ModelEntry {
    /// Resource name in the official `models/<id>` form.
    name: String,
}

// Streaming query marker: only the `alt=sse` form is supported. The query is parsed
// manually from the request URI (see `has_alt_sse`) without the axum `query` feature,
// so no extractor type is declared.

/// Body extractor rejection mapped into the surface's Gemini error shape: an over-limit
/// body renders the surface's 413, every other body failure renders as a malformed
/// request.
#[derive(Debug)]
struct BodyError(Error);

impl From<BytesRejection> for BodyError {
    /// Converts an axum body rejection into the surface error domain.
    fn from(rejection: BytesRejection) -> Self {
        Self(Error::from(rejection))
    }
}

impl IntoResponse for BodyError {
    /// Renders the body failure through the prefix Gemini error shape, logging once at
    /// this boundary.
    fn into_response(self) -> Response {
        log_error("request body", &self.0);

        super::error_response(self.0)
    }
}

/// Handles `GET /models`: the native `{"models": [...]}` shape over the same
/// effective-model set every surface lists, ignoring pagination parameters and returning
/// the full list in one page with no token.
async fn models(State(context): State<Context>) -> Response {
    match storage::list_enabled_models_with_owner(&context.database).await {
        Ok(listed) => {
            let models = listed
                .iter()
                .map(|(public_id, _)| ModelEntry {
                    name: format!("models/{public_id}"),
                })
                .collect();

            super::json_response(StatusCode::OK, &ModelList { models })
        }

        Err(error) => {
            let error = Error::from(error);

            log_error("model listing", &error);

            super::error_response(error)
        }
    }
}

/// Handles both completion actions behind the wildcard route: parses the `rest` capture
/// into its model plus action, enforces the streaming query convention, then runs the
/// shared completion pipeline.
async fn dispatch(
    State(context): State<Context>,
    api_key: super::AuthenticatedApiKeyRow,
    OriginalUri(uri): OriginalUri,
    WithRejection(body, _): WithRejection<Bytes, BodyError>,
) -> Response {
    let path = uri.path();

    let rest = path
        .strip_prefix("/api/gemini-v1beta/models/")
        .or_else(|| path.strip_prefix("/models/"))
        .unwrap_or("");

    // The wildcard capture is everything after `/models/`; it is re-derived from the
    // request path rather than through a typed extractor so malformed paths stay in
    // this handler's 404 domain instead of the router's.
    let action = match parse_model_action(rest) {
        Ok(action) => action,

        Err(error) => {
            log_error("request", &error);

            return super::error_response(error);
        }
    };

    if action.is_streaming {
        let is_sse = has_alt_sse(uri.query());

        if !is_sse {
            let error = Error::MalformedRequest {
                param: Some("alt".to_string()),
                message: "streamGenerateContent requires `?alt=sse`".to_string(),
            };

            log_error("request", &error);

            return super::error_response(error);
        }
    }

    outcome_response(
        handle_completion(
            &context,
            &body,
            api_key.0,
            action.model,
            action.is_streaming,
        )
        .await,
    )
}

/// Reports whether a query string carries `alt=sse`: the only supported streaming
/// query form. The check parses the raw query without the `query` extractor (which
/// would require the axum `query` feature) by splitting on `&` and `=`.
fn has_alt_sse(query: Option<&str>) -> bool {
    let Some(query) = query else {
        return false;
    };

    query.split('&').any(|pair| {
        let mut parts = pair.splitn(2, '=');

        parts.next() == Some("alt") && parts.next() == Some("sse")
    })
}

/// Renders a handler outcome, mapping failures to Gemini error responses.
fn outcome_response(outcome: Result<Response, Error>) -> Response {
    match outcome {
        Ok(response) => response,
        Err(error) => super::error_response(error),
    }
}

/// One parsed wildcard action: the URL-routed model plus the streaming mode from the URL
/// action, not the body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelAction {
    /// Public model id from the URL path.
    model: String,

    /// Streaming mode from the URL action.
    is_streaming: bool,
}

/// Parses the wildcard capture after `/models/` into its model plus action.
///
/// The capture must be `<model>:generateContent` or `<model>:streamGenerateContent`
/// with a non-empty model containing neither `/` nor `:` (such rows are not addressable
/// through this surface; README §6.5). Anything else is an unmapped path and returns
/// the surface 404, so malformed paths observe the same body the prefix fallback
/// renders.
///
/// # Errors
///
/// Returns [`Error::InvalidUrl`] for captures outside the two action shapes.
fn parse_model_action(rest: &str) -> Result<ModelAction, Error> {
    // A `/` inside the capture means the model segment itself contained a slash: the
    // URL path cannot carry such a public ID, so the path is unmapped.
    if rest.contains('/') {
        return Err(Error::InvalidUrl {
            method: Method::POST.as_str().to_string(),
            path: format!("/api/gemini-v1beta/models/{rest}"),
        });
    }

    let (model, action) = rest.split_once(':').ok_or_else(|| Error::InvalidUrl {
        method: Method::POST.as_str().to_string(),
        path: format!("/api/gemini-v1beta/models/{rest}"),
    })?;

    if model.is_empty() || model.contains(':') {
        return Err(Error::InvalidUrl {
            method: Method::POST.as_str().to_string(),
            path: format!("/api/gemini-v1beta/models/{rest}"),
        });
    }

    match action {
        "generateContent" => Ok(ModelAction {
            model: model.to_string(),
            is_streaming: false,
        }),

        "streamGenerateContent" => Ok(ModelAction {
            model: model.to_string(),
            is_streaming: true,
        }),

        _ => Err(Error::InvalidUrl {
            method: Method::POST.as_str().to_string(),
            path: format!("/api/gemini-v1beta/models/{rest}"),
        }),
    }
}

/// Routes one completion request through the gateway in the GenerateContent client
/// format: parse to IR, resolve routing, execute upstream, and render the event stream
/// live or aggregated.
///
/// Every failure that reaches this boundary is logged exactly once here, with the
/// request context available at each stage: a parse rejection names only the endpoint,
/// a routing rejection names the requested model, and everything past routing
/// additionally names the resolved provider.
///
/// `api_key_id` — extracted from the authentication middleware's stamp — anchors the
/// request's accounting draft, which this function builds from the routing facts and
/// hands to [`gateway::execute`] for the dispatch mark. Exactly one of the three
/// finalization points then records the row: [`gateway::execute`] itself for a
/// post-dispatch failure, the aggregation path below for a non-streaming response, or
/// the SSE render task for a streaming one (README §3 *ApiLog*).
///
/// # Errors
///
/// Returns [`Error`] for request, routing, and upstream failures.
async fn handle_completion(
    context: &Context,
    body: &[u8],
    api_key_id: i64,
    model: String,
    is_streaming_route: bool,
) -> Result<Response, Error> {
    let route = RouteContext::with_model(model, Some(is_streaming_route));

    let parsed = match InboundFormat::parse_request(body, &route) {
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

    // The accounting draft: every fact known before dispatch, built from the
    // authenticated key and the routing row; `gateway::execute` stamps the dispatch
    // instant into it (README §3 *ApiLog*).
    let mut log = ApiLogDraft {
        started_at: None,
        api_key_id,
        provider_id: routing.provider.id,
        model_id: routing.model.id,
        endpoint_type: InboundFormat::ENDPOINT_TYPE,
        is_stream: is_streaming,
    };

    let outcome = async {
        let events = gateway::execute(context, routing, parsed.ir, is_streaming, &mut log).await?;

        if is_streaming {
            Ok(sse_response(
                events,
                InboundFormat::sse_renderer(should_include_usage, public_id),
                context.database.clone(),
                log,
            ))
        } else {
            // Finalization point 2: the non-streaming path records the row synchronously
            // after aggregation and body rendering, before the Response leaves —
            // `completed` + 200 on success, the classified outcome and status on a
            // stream failure (README §3 *ApiLog*).
            match aggregated_response(events, |aggregation| {
                InboundFormat::render_json_response(&public_id, aggregation)
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

/// Wraps an IR event stream into a live `text/event-stream` response, carrying the
/// request's accounting draft and database handle into the render task: that task is
/// the streaming finalization point (README §3 *ApiLog*) and records exactly one row
/// when the stream closes — `completed` + 200 for a clean end, the classified outcome
/// under the already-declared 200 for an error frame (the first enqueued frame
/// declares the status, so an error frame never changes it), and `client_aborted`
/// for a broken body channel — with 200 only once a frame had been enqueued and
/// `NULL` before the first. The recorded usage is whatever the stream delivered
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

        // Whether any frame reached the client's channel; the first one declares HTTP
        // 200.
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
                            // The client's body channel is gone: a client abort, keyed
                            // only on what was actually enqueued before it broke.
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
                        // The channel broke before the error frame could be enqueued:
                        // the client left, so the disconnect rules decide alone.
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

                    // The error frame never changes the status already declared; only
                    // the outcome moves to the classified failure (README §3 *ApiLog*).
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

/// The status recorded for a stream close: 200 once any frame — an error frame
/// included — was enqueued, because the first enqueued frame is what declares the
/// response to the client; `NULL` before the first, when no response ever reached it
/// (README §3 *ApiLog*).
fn declared_status(is_status_declared: bool) -> Option<i64> {
    if is_status_declared {
        Some(i64::from(StatusCode::OK.as_u16()))
    } else {
        None
    }
}

/// Folds an IR event stream into one aggregated JSON response, yielding the response
/// beside the terminal usage the aggregation observed — `None` when the stream
/// carried no usage report, and always `None` for a failed stream, whose error
/// carries no terminal usage.
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
        super::json_response(StatusCode::OK, &render_json_fn(&aggregation)),
        usage,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the wildcard capture parsing: the two action shapes route with their
    /// streaming modes, while every other shape — missing or empty model, unknown
    /// action, extra segments, or a model carrying routing-unsafe characters — is an
    /// unmapped path returning the surface 404.
    ///
    /// # Panics
    ///
    /// Panics if a capture parses outside its documented outcome.
    #[test]
    fn parses_wildcard_captures_into_model_actions() {
        for (rest, model, is_streaming) in [
            ("gemini-2.0:generateContent", "gemini-2.0", false),
            ("gemini-2.0:streamGenerateContent", "gemini-2.0", true),
        ] {
            #[expect(
                clippy::expect_used,
                reason = "each valid capture is constructed to parse"
            )]
            let action = parse_model_action(rest).expect("a valid capture must parse");

            assert_eq!(
                action,
                ModelAction {
                    model: model.to_string(),
                    is_streaming,
                }
            );
        }

        for rest in [
            "",
            ":generateContent",
            "model:unknownAction",
            "modelgenerateContent",
            "a/b:generateContent",
            "a:b:generateContent",
            "model:",
        ] {
            #[expect(
                clippy::expect_used,
                reason = "each invalid capture is constructed to fail"
            )]
            let error = parse_model_action(rest).expect_err("an invalid capture must 404");

            assert!(
                matches!(error, Error::InvalidUrl { .. }),
                "unexpected error for `{rest}`: {error}"
            );
            assert_eq!(error.http_status(), StatusCode::NOT_FOUND);
        }
    }

    /// Verifies the router builds without panicking: axum 0.8 rejects partial-segment
    /// parameters (`/models/{model}:generateContent` panics with "Only one parameter is
    /// allowed per path segment"), so the wildcard route above is the supported shape
    /// and this test pins that it stays buildable.
    ///
    /// # Panics
    ///
    /// Panics if the router fails to build.
    #[test]
    fn router_builds_with_wildcard_routes() {
        let _router: Router<Context> = router();
    }

    /// Verifies the native model-listing shape renders `models/<id>` names with no
    /// pagination token.
    ///
    /// # Panics
    ///
    /// Panics if the rendered shape diverges from the native contract.
    #[test]
    fn renders_native_model_listing_shape() {
        let listed = [
            ("b-model".to_string(), "b-provider".to_string()),
            ("a-model".to_string(), "a-provider".to_string()),
        ];

        let models = listed
            .iter()
            .map(|(public_id, _)| ModelEntry {
                name: format!("models/{public_id}"),
            })
            .collect();

        #[expect(clippy::expect_used, reason = "the listing body always serializes")]
        let value = serde_json::to_value(&ModelList { models }).expect("the listing serializes");

        assert_eq!(
            value,
            serde_json::json!({
                "models": [
                    {"name": "models/b-model"},
                    {"name": "models/a-model"},
                ],
            })
        );
        assert!(value.get("nextPageToken").is_none());
    }
}
