//! The shared outward serving pipeline behind every `/api` format surface (README §2.2).
//!
//! One home for the machinery the four surfaces (`/openai-v1`, `/gemini-v1beta` GenerateContent,
//! `/gemini-v1beta` Interactions, `/anthropic-v1`) previously each carried a copy of: the
//! completion pipeline [`run_completion`], the SSE and aggregated response machinery with the
//! request-accounting finalization rules, the body-extractor rejection, the authentication
//! middleware, the 404/405 fallbacks, and the two pipeline constants. This duplication is README
//! §3's accounting invariant itself — exactly three finalization points (dispatch failure in
//! [`gateway::execute`], the aggregation path, the SSE render task), the declared-status rule,
//! and the client-abort classification — so it is single-sourced here and every surface renders
//! through the same code.
//!
//! What stays per surface is exactly its identity: the router, the authentication convention
//! (which headers carry the credential), the native model-list shape, and the error wire
//! renderer. Each surface states that identity by implementing [`Surface`] for a private marker
//! type and pointing its middleware, fallbacks, and handlers at the shared functions through it;
//! [`run_completion`] itself is [`ClientFormat`]-generic and needs no surface identity.

use crate::{
    api::{Error, log_error},
    context::Context,
    database::Database,
    format::{ClientFormat, ClientSseRenderer, RouteContext},
    gateway::{self, IrEventStream},
    ir::{Aggregator, IrAggregation, IrEvent, IrUsage},
    security,
    storage::{
        api_keys,
        api_logs::{self, ApiLogDraft, ApiLogOutcome},
    },
};
use axum::{
    body::{Body, Bytes},
    extract::{FromRequestParts, OriginalUri, Request, State, rejection::BytesRejection},
    http::{Method, StatusCode, header, request::Parts as RequestParts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::{convert::Infallible, io::Error as IoError, marker::PhantomData};
use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

/// Maximum accepted request body size, large enough for inline image data.
pub(super) const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Capacity of the channel between the SSE rendering task and the response body stream.
///
/// Mirrors `gateway`'s `EVENT_CHANNEL_BUFFER` for the adjacent pipeline stage: both hops buffer
/// equally, so one slow stage cannot accumulate an asymmetric backlog in the other.
pub(super) const SSE_CHANNEL_BUFFER: usize = 64;

/// The per-surface identity the shared pipeline renders through: the two conventions that are
/// deliberately not shared — how a request presents its credential, and how an [`Error`] lands on
/// the wire.
///
/// Every surface module implements this trait for a private marker type and points its router's
/// authentication middleware, 404/405 fallbacks, and handlers at [`authenticate`], [`fallback`],
/// [`method_not_allowed`], [`BodyError`], and [`outcome_response`] through it.
pub(super) trait Surface {
    /// Renders one API failure as the surface's JSON error envelope: the surface's error wire
    /// renderer, through which every rejection and handler failure the pipeline produces lands on
    /// the wire.
    fn render_error(error: Error) -> Response;

    /// Reads the presented credential key from the request per the surface's authentication
    /// convention — the header that carries it and any companion header the convention requires —
    /// or classifies why the convention cannot accept the request's credential, having logged that
    /// rejection at the surface's boundary. The returned string is the raw presented secret;
    /// matching it against the enabled keys and rendering the rejection both stay in the shared
    /// [`authenticate`].
    fn extract_credential(request: &Request) -> Result<String, AuthRejection>;

    /// Records a presented key that matched no enabled gateway API key at the surface's log
    /// boundary, just before the shared [`authenticate`] renders its rejection.
    fn log_invalid_key();
}

/// Why a request's credential failed the surface's authentication convention before any key
/// comparison ([`Surface::extract_credential`]): no credential the convention could read, or a
/// convention requirement beyond the credential header the request failed. The shared
/// [`authenticate`] renders both through the surface's error envelope.
pub(super) enum AuthRejection {
    /// The request presented no credential the surface's convention could read; the shared
    /// pipeline classifies it as the shared invalid-key rejection.
    MissingCredential,

    /// The request failed a convention requirement beyond the credential header itself; the
    /// already-classified contract violation renders as is.
    Malformed(Error),
}

/// The row id of the gateway API key that authorized the current request: the authentication
/// middleware stamps it into the request extensions when it lets a request through, and the
/// completion handlers extract it to attribute the request's accounting row (README §3 *ApiLog*).
/// Only the never-recycled row identity travels this far — never the secret itself.
#[derive(Debug, Clone, Copy)]
pub(super) struct AuthenticatedApiKeyRow(pub(super) i64);

impl<S> FromRequestParts<S> for AuthenticatedApiKeyRow
where
    S: Send + Sync,
{
    /// Never constructed: this extractor fails fast instead of rejecting. A completion handler
    /// always runs behind [`authenticate`] first, so a missing extension is an invariant
    /// violation of the router's own pipeline, not a client-repairable condition.
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut RequestParts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        #[expect(
            clippy::expect_used,
            reason = "the authentication middleware always stamps this extension before a \
                      completion handler runs; a missing extension is an invariant violation \
                      that fails fast"
        )]
        let row_id = parts
            .extensions
            .get::<AuthenticatedApiKeyRow>()
            .expect("an authenticated completion request carries its API key row id")
            .0;

        Ok(Self(row_id))
    }
}

/// Authenticates the request against the enabled gateway API keys, reading the presented
/// credential through the surface's convention ([`Surface::extract_credential`]). The keys are
/// read from the live database, so a created, rotated, or disabled key takes effect on the next
/// request; a database failure renders the surface's internal error instead of an authentication
/// rejection. A request that passes is stamped with the matching key's row id, which the
/// completion handlers carry into the request accounting (README §3 *ApiLog*).
pub(super) async fn authenticate<S>(
    State(database): State<Database>,
    request: Request,
    next: Next,
) -> Response
where
    S: Surface,
{
    let presented = match S::extract_credential(&request) {
        Ok(presented) => presented,

        Err(rejection) => {
            let error = match rejection {
                AuthRejection::MissingCredential => Error::InvalidApiKey,

                AuthRejection::Malformed(error) => error,
            };

            return S::render_error(error);
        }
    };

    match api_keys::list_enabled_api_key_secrets(&database).await {
        Ok(keys) => {
            let matched = keys
                .into_iter()
                .find(|(_, secret)| security::constant_time_eq(&presented, secret));

            match matched {
                Some((row_id, _secret)) => {
                    let mut request = request;

                    request
                        .extensions_mut()
                        .insert(AuthenticatedApiKeyRow(row_id));

                    next.run(request).await
                }

                None => {
                    S::log_invalid_key();

                    S::render_error(Error::InvalidApiKey)
                }
            }
        }

        Err(error) => {
            let error = Error::from(error);

            log_error("request", &error);

            S::render_error(error)
        }
    }
}

/// Handles unmapped surface paths with the surface's 404, logging the rendered error once at this
/// response boundary.
pub(super) async fn fallback<S>(OriginalUri(uri): OriginalUri, request: Request) -> Response
where
    S: Surface,
{
    let error = Error::InvalidUrl {
        method: request.method().as_str().to_string(),
        path: uri.path().to_string(),
    };

    log_error("request", &error);

    S::render_error(error)
}

/// Handles a request whose method is not registered on an otherwise-matching path with the
/// surface's 405, logging the rendered error once at this response boundary. Axum still attaches
/// the `Allow` header listing the registered methods.
pub(super) async fn method_not_allowed<S>(method: Method, OriginalUri(uri): OriginalUri) -> Response
where
    S: Surface,
{
    let error = Error::MethodNotAllowed;

    log_error(
        &format!("request {} {}", method.as_str(), uri.path()),
        &error,
    );

    S::render_error(error)
}

/// Body extractor rejection mapped into the surface's error shape: an over-limit body renders the
/// surface's 413, every other body failure renders as a malformed request.
///
/// The surface parameter rides as `fn() -> S`, which is unconditionally `Send` and `Sync`
/// regardless of `S`: the marker contributes nothing at runtime, and the response body must stay
/// `Send`.
#[derive(Debug)]
pub(super) struct BodyError<S>(Error, PhantomData<fn() -> S>);

impl<S> From<BytesRejection> for BodyError<S> {
    fn from(rejection: BytesRejection) -> Self {
        Self(Error::from(rejection), PhantomData)
    }
}

impl<S> IntoResponse for BodyError<S>
where
    S: Surface,
{
    fn into_response(self) -> Response {
        log_error("request body", &self.0);

        S::render_error(self.0)
    }
}

/// Routes one completion request through the gateway in the given client format: parse to IR,
/// resolve routing, execute upstream, and render the event stream live or aggregated.
///
/// `route` is the URL-path facts the request's format parses under ([`RouteContext`]): empty on
/// the body-routed surfaces, the public model id (and streaming mode) on the Gemini
/// GenerateContent surface, the streaming override on Interactions. `operation` names the request
/// kind in every log line the pipeline records (`"completion"` or `"interaction"`), composed into
/// `{operation} request`, `{operation} for model …`, and `{operation} for model … via provider …`.
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
pub(super) async fn run_completion<TFormat>(
    context: &Context,
    body: &[u8],
    api_key_id: i64,
    route: RouteContext,
    operation: &str,
) -> Result<Response, Error>
where
    TFormat: ClientFormat,
    TFormat::JsonResponse: Serialize,
{
    let parsed = match TFormat::parse_request(body, &route) {
        Ok(parsed) => parsed,

        Err(error) => {
            let error = Error::from(error);

            log_error(&format!("{operation} request"), &error);

            return Err(error);
        }
    };

    let routing = match gateway::resolve_routing(&context.database, &parsed.ir.model).await {
        Ok(routing) => routing,

        Err(error) => {
            let error = Error::from(error);

            log_error(
                &format!("{operation} for model `{}`", parsed.ir.model),
                &error,
            );

            return Err(error);
        }
    };

    let operation = format!(
        "{operation} for model `{}` via provider `{}`",
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

/// Renders a handler outcome, mapping failures to the surface's error responses.
pub(super) fn outcome_response<S>(outcome: Result<Response, Error>) -> Response
where
    S: Surface,
{
    match outcome {
        Ok(response) => response,

        Err(error) => S::render_error(error),
    }
}

/// Builds a JSON response with the given status code.
///
/// The outward surface's universal response builder, defined once beside the shared pipeline so
/// every surface owns the same rendering. The body is a typed payload, so serialization is total.
///
/// # Panics
///
/// Panics if the rendered body fails to serialize or the static response
/// parts fail to build, neither of which can occur.
pub(super) fn json_response<T>(status: StatusCode, body: &T) -> Response
where
    T: Serialize,
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
