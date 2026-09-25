//! The shared outward serving pipeline behind every `/api` format surface (README §2.2).
//!
//! One home for the machinery the four surfaces (`/openai-v1`, `/gemini-v1beta` GenerateContent,
//! `/gemini-v1beta` Interactions, `/anthropic-v1`) previously each carried a copy of: the
//! completion pipeline [`run_completion`], the SSE and aggregated response machinery with the
//! request-accounting finalization rules, the body-extractor rejection, the authentication
//! middleware, the 404/405 fallbacks, and the two pipeline constants. This duplication is README
//! §3's accounting invariant itself — exactly three finalization points (dispatch failure in
//! [`gateway::execute`], the detached non-streaming aggregation task, the SSE render task), the
//! declared-status rule, and the client-abort classification — so it is single-sourced here and
//! every surface renders through the same code.
//!
//! What stays per surface is exactly its identity: the router, the authentication convention
//! (which headers carry the credential), the native model-list shape, and the error wire
//! renderer. Each surface states that identity by implementing [`Surface`] for a private marker
//! type and pointing its middleware, fallbacks, and handlers at the shared functions through it;
//! [`run_completion`] itself is [`ClientFormat`]-generic and needs no surface identity.

use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome},
    api::{Error, log_error},
    context::Context,
    database::Database,
    format::{ClientFormat, ClientSseRenderer, RouteContext},
    gateway::{self, IrEventStream},
    ir::{Aggregator, IrAggregation, IrEvent, IrUsage},
    security,
    storage::{
        api_keys,
        api_logs::{self, ApiLogDraft},
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
/// authentication middleware, 404/405 fallbacks, and handlers at [`authenticate`],
/// [`error_response`], [`fallback`], [`method_not_allowed`], [`BodyError`], and
/// [`outcome_response`] through it.
pub(super) trait Surface {
    /// Renders one API failure onto the surface's error wire: the surface's error render module's
    /// `render` function, which classifies the [`Error`] variant into the surface's (HTTP status,
    /// JSON envelope) pair (README §6.7). The shared [`error_response`] composes it with the
    /// universal JSON response builder, so every rejection and handler failure the pipeline
    /// produces lands on the wire through one glue path.
    fn render_error(error: &Error) -> (StatusCode, impl Serialize);

    /// Reads the presented credential key from the request per the surface's authentication
    /// convention — the header that carries it and any companion header the convention requires —
    /// or classifies why the convention cannot accept the request's credential, having logged that
    /// rejection at the surface's boundary. The returned string is the raw presented secret;
    /// matching it against the enabled keys and rendering the rejection both stay in the shared
    /// [`authenticate`].
    fn extract_credential(request: &Request) -> Result<String, AuthRejection>;
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

            return error_response::<S>(error);
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
                    // A presented key that matched no enabled gateway API key: logged once at
                    // this shared boundary, then rendered as the invalid-key rejection.
                    let error = Error::InvalidApiKey;

                    log_error("request", &error);

                    error_response::<S>(error)
                }
            }
        }

        Err(error) => {
            let error = Error::from(error);

            log_error("request", &error);

            error_response::<S>(error)
        }
    }
}

/// Renders one API failure as the surface's JSON error response: the surface's error wire
/// renderer ([`Surface::render_error`]) through the universal JSON response builder
/// [`json_response`]. Every rejection and handler failure the pipeline produces lands on the wire
/// here, and each surface's handlers reuse it directly for their own failures (README §6.7).
pub(super) fn error_response<S>(error: Error) -> Response
where
    S: Surface,
{
    let (status, envelope) = S::render_error(&error);

    json_response(status, &envelope)
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

    error_response::<S>(error)
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

    error_response::<S>(error)
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

        error_response::<S>(self.0)
    }
}

/// The closed request-kind vocabulary [`run_completion`] names in its log lines: a completion —
/// every surface's endpoint but one — or a Gemini Interactions interaction (`POST
/// /api/gemini-v1beta/interactions`). [`OperationKind::as_str`] renders the exact log spelling,
/// so the composed log templates stay one vocabulary with the call sites.
#[derive(Debug, Clone, Copy)]
pub(super) enum OperationKind {
    /// A completion request: any surface endpoint except Gemini Interactions.
    Completion,

    /// A Gemini Interactions interaction request (`POST /interactions`).
    Interaction,
}

impl OperationKind {
    /// Renders the log-line spelling of the request kind.
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Completion => "completion",
            Self::Interaction => "interaction",
        }
    }
}

/// Routes one completion request through the gateway in the given client format: parse to IR,
/// resolve routing, execute upstream, and render the event stream live or aggregated.
///
/// `route` is the URL-path facts the request's format parses under ([`RouteContext`]): empty on
/// the body-routed surfaces, the public model id (and streaming mode) on the Gemini
/// GenerateContent surface, the streaming override on Interactions. `operation` names the request
/// kind in every log line the pipeline records ([`OperationKind`]), composed through
/// [`OperationKind::as_str`] into `{operation} request`, `{operation} for model …`, and
/// `{operation} for model … via provider …`. `endpoint_type` names the outward completion
/// endpoint the request arrived on, recorded in the request's accounting row (README §3
/// *ApiLog*) — each call site names its own surface's endpoint, the way `operation` names the
/// request kind.
///
/// Every failure that reaches this boundary is logged exactly once here, with the request context
/// available at each stage: a parse rejection names only the endpoint, a routing rejection names
/// the requested model, and everything past routing additionally names the resolved provider.
///
/// `api_key_id` — extracted from the authentication middleware's stamp — anchors the request's
/// accounting draft, which this function builds from the routing facts and hands to
/// [`gateway::execute`] for the dispatch mark. Exactly one of the three finalization points then
/// records the row: [`gateway::execute`] itself for a post-dispatch failure, the detached
/// non-streaming aggregation task for a non-streaming response, or the SSE render task for a
/// streaming one (README §3 *ApiLog*). The non-streaming task outlives this handler — hyper
/// drops the handler the moment the client departs — so a departure mid-generation finalizes the
/// row as `client_aborted` instead of losing it, and a departure after the aggregated body was
/// handed off stays the accepted unobservable edge (§3 *ApiLog*).
///
/// # Panics
///
/// Panics if the detached aggregation task does not hand its outcome to the handler. The task
/// hands the outcome on every path, so only the task's own panic can cause it: the expect
/// converts that invariant violation into this handler's own panic, per the fail-fast design.
///
/// # Errors
///
/// Returns [`Error`] for request, routing, and upstream failures.
pub(super) async fn run_completion<TFormat>(
    context: &Context,
    body: &[u8],
    api_key_id: i64,
    route: RouteContext,
    operation: OperationKind,
    endpoint_type: ApiLogEndpointType,
) -> Result<Response, Error>
where
    // `'static` because the non-streaming aggregation task, which renders through `TFormat`,
    // is spawned detached.
    TFormat: ClientFormat + 'static,
    TFormat::JsonResponse: Serialize,
{
    let parsed = match TFormat::parse_request(body, &route) {
        Ok(parsed) => parsed,

        Err(error) => {
            let error = Error::from(error);

            log_error(&format!("{} request", operation.as_str()), &error);

            return Err(error);
        }
    };

    let routing = match gateway::resolve_routing(&context.database, &parsed.ir.model).await {
        Ok(routing) => routing,

        Err(error) => {
            let error = Error::from(error);

            log_error(
                &format!("{} for model `{}`", operation.as_str(), parsed.ir.model),
                &error,
            );

            return Err(error);
        }
    };

    let log_context = format!(
        "{} for model `{}` via provider `{}`",
        operation.as_str(),
        routing.model.public_id,
        routing.provider.name
    );

    let public_id = routing.model.public_id.clone();

    let is_streaming = parsed.is_streaming;

    let should_include_usage = parsed.should_include_usage;

    // The accounting draft: every fact known before dispatch, built from the authenticated key,
    // the routing row, and the surface's endpoint type; `gateway::execute` stamps the dispatch
    // instant into it (README §3 *ApiLog*).
    let mut log = ApiLogDraft {
        started_at: None,
        api_key_id,
        provider_id: routing.provider.id,
        model_id: routing.model.id,
        endpoint_type,
        is_stream: is_streaming,
    };

    let outcome = async {
        let events = gateway::execute(context, routing, parsed.ir, is_streaming, &mut log).await?;

        if is_streaming {
            Ok(sse_response(
                events,
                TFormat::sse_renderer(should_include_usage, public_id),
                context.api_log_store.clone(),
                log,
            ))
        } else {
            // Finalization point 2: the non-streaming path detaches its aggregation into a task
            // that owns the event stream and the accounting draft (README §3 *ApiLog*). The task
            // consumes the stream to completion, renders the aggregated body, and hands the
            // outcome to this handler through the oneshot; the hand-off attempt is the
            // client-liveness signal that decides the row's outcome, so the row finalizes even
            // when hyper has already dropped this handler with the departing connection.
            let (outcome_sender, outcome_receiver) = tokio::sync::oneshot::channel();

            let aggregation = tokio::spawn(aggregated_response_task::<TFormat>(
                events,
                public_id,
                context.api_log_store.clone(),
                log,
                log_context.clone(),
                outcome_sender,
            ));

            // Detaching the handle is the point: the task must outlive this handler, which is
            // dropped the moment the client departs, and it carries the row's finalization.
            drop(aggregation);

            #[expect(
                clippy::expect_used,
                reason = "the aggregation task always hands its outcome to the handler exactly \
                          once; a missing hand-off is an invariant violation that fails fast"
            )]
            let outcome = outcome_receiver
                .await
                .expect("the aggregation task always hands its outcome to the handler");

            outcome
        }
    }
    .await;

    if let Err(error) = &outcome {
        log_error(&log_context, error);
    }

    outcome
}

/// The detached non-streaming aggregation task (finalization point 2, README §3 *ApiLog*):
/// consumes the upstream event stream to completion, renders the aggregated body, and hands
/// `Ok(response)` or `Err(error)` to the handler through `outcome_sender` — the handler renders
/// it as the request's HTTP response when it is still there. The task owns the request's
/// accounting draft, so the row finalizes even when hyper dropped the handler with the departing
/// connection, and the hand-off attempt is the client-liveness signal:
///
/// - a successful stream finalizes `completed` + 200 + usage when the hand-off reaches the handler,
///   and `client_aborted` + `NULL` status when it does not — no response was ever produced for a
///   departed client;
/// - a failed stream finalizes the outcome and status the shared [`Error`] classification yields,
///   regardless of the client — the upstream failure, not the departure, is what the row records.
async fn aggregated_response_task<TFormat>(
    events: IrEventStream,
    public_id: String,
    api_log_store: Option<api_logs::ApiLogStore>,
    log: ApiLogDraft,
    log_context: String,
    outcome_sender: tokio::sync::oneshot::Sender<Result<Response, Error>>,
) where
    TFormat: ClientFormat,
    TFormat::JsonResponse: Serialize,
{
    let aggregation = aggregated_response(events, |aggregation| {
        TFormat::render_json_response(&public_id, aggregation)
    })
    .await;

    match aggregation {
        Ok((response, usage)) => {
            if outcome_sender.send(Ok(response)).is_ok() {
                // The handler received the response: the client was still there.
                api_logs::finalize_api_log(
                    api_log_store.as_ref(),
                    &log,
                    ApiLogOutcome::Completed,
                    Some(i64::from(StatusCode::OK.as_u16())),
                    usage,
                )
                .await;
            } else {
                // The handler is gone — the client departed mid-generation — and no response was
                // ever produced for it.
                api_logs::finalize_api_log(
                    api_log_store.as_ref(),
                    &log,
                    ApiLogOutcome::ClientAborted,
                    None,
                    usage,
                )
                .await;
            }
        }

        Err(error) => {
            // An upstream failure records its classification regardless of the client.
            let (outcome, http_status) = error.accounting();

            api_logs::finalize_api_log(
                api_log_store.as_ref(),
                &log,
                outcome,
                Some(http_status),
                None,
            )
            .await;

            // A failed hand-off returns the outcome: the handler is gone, so no response
            // boundary will render this failure and this task is its single logger (README §8).
            if let Err(Err(error)) = outcome_sender.send(Err(error)) {
                log_error(&log_context, &error);
            }
        }
    }
}

/// Renders a handler outcome, mapping failures to the surface's error responses.
pub(super) fn outcome_response<S>(outcome: Result<Response, Error>) -> Response
where
    S: Surface,
{
    match outcome {
        Ok(response) => response,

        Err(error) => error_response::<S>(error),
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
/// accounting draft and shard store handle into the render task: that task is the streaming
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
    api_log_store: Option<api_logs::ApiLogStore>,
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
                                api_log_store.as_ref(),
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
                            api_log_store.as_ref(),
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
                        api_log_store.as_ref(),
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
            api_log_store.as_ref(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        format::openai_chat_completion::Format as ChatCompletionFormat, ir::IrFinishReason,
        storage::api_logs::list_api_logs,
    };
    use std::{num::NonZero, path::PathBuf};
    use tokio::sync::{mpsc, oneshot};

    /// Capacity of the fixture event channel: wide enough that every fixture send completes
    /// without awaiting the consumer.
    const EVENT_CHANNEL_BUFFER: usize = 8;

    /// Directory guard cleaning up the temporary shard tree on drop.
    struct TempStoreGuard(PathBuf);

    impl Drop for TempStoreGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Creates a shard store over a fresh temporary directory, with its cleanup guard.
    ///
    /// # Panics
    ///
    /// Panics if the literal retention is zero, which the positive literal prevents.
    fn test_store() -> (api_logs::ApiLogStore, TempStoreGuard) {
        let directory =
            std::env::temp_dir().join(format!("janusgate-serving-test-{}", rand::random::<u64>()));

        #[expect(clippy::unwrap_used, reason = "non-zero test retention")]
        let retention = NonZero::new(90).unwrap();

        (
            api_logs::ApiLogStore::new(directory.clone(), retention),
            TempStoreGuard(directory),
        )
    }

    /// The stamped accounting draft fixture: fixed identity ids, a fixed dispatch stamp, and the
    /// non-streaming flag — the shape the draft carries once [`gateway::execute`] has stamped it
    /// at dispatch.
    fn stamped_draft() -> ApiLogDraft {
        ApiLogDraft {
            started_at: Some(1_000_000_000),
            api_key_id: 3,
            provider_id: 5,
            model_id: 7,
            endpoint_type: ApiLogEndpointType::OpenaiChatCompletions,
            is_stream: false,
        }
    }

    /// The response-boundary log context fixture, composed as [`run_completion`] composes it.
    fn log_context() -> String {
        "completion for model `public-model` via provider `acme`".to_string()
    }

    /// Opens the fake upstream event channel the task under test consumes: the sender rides in
    /// the test, and the receiving stream is boxed exactly as [`gateway::execute`] returns it.
    fn event_channel() -> (mpsc::Sender<Result<IrEvent, gateway::Error>>, IrEventStream) {
        let (sender, receiver) = mpsc::channel(EVENT_CHANNEL_BUFFER);

        (sender, Box::pin(ReceiverStream::new(receiver)))
    }

    /// Sends one clean completion's event sequence — start, text, usage, finish, done — through
    /// the fixture channel and closes the stream.
    ///
    /// # Panics
    ///
    /// Panics if a fixture send fails, which the wide buffer and the task-owned receiver prevent.
    async fn send_clean_events(sender: &mpsc::Sender<Result<IrEvent, gateway::Error>>) {
        let events = [
            IrEvent::MessageStart {
                id: "resp-1".to_string(),
                model: "upstream-model".to_string(),
            },
            IrEvent::TextDelta("hello".to_string()),
            IrEvent::Usage(IrUsage {
                input_tokens: 11,
                output_tokens: 7,
                cached_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            }),
            IrEvent::Finish(IrFinishReason::Stop),
            IrEvent::Done,
        ];

        for event in events {
            #[expect(
                clippy::expect_used,
                reason = "the fixture channel is wide enough and its receiver lives in the task \
                          under test"
            )]
            sender
                .send(Ok(event))
                .await
                .expect("the fixture channel accepts the event");
        }
    }

    /// Sends a mid-stream upstream failure — one start event, the transport failure, the pump's
    /// terminal `Done` — through the fixture channel and closes the stream.
    ///
    /// # Panics
    ///
    /// Panics if a fixture send fails, which the wide buffer and the task-owned receiver prevent.
    async fn send_failing_events(sender: &mpsc::Sender<Result<IrEvent, gateway::Error>>) {
        let events = [
            Ok(IrEvent::MessageStart {
                id: "resp-1".to_string(),
                model: "upstream-model".to_string(),
            }),
            Err(gateway::Error::Unavailable {
                detail: "upstream stream failed: broken".to_string(),
            }),
            Ok(IrEvent::Done),
        ];

        for event in events {
            #[expect(
                clippy::expect_used,
                reason = "the fixture channel is wide enough and its receiver lives in the task \
                          under test"
            )]
            sender
                .send(event)
                .await
                .expect("the fixture channel accepts the event");
        }
    }

    /// Reads every recorded row of `store`.
    ///
    /// # Panics
    ///
    /// Panics if the read fails, which a healthy fixture store prevents.
    async fn recorded_rows(store: &api_logs::ApiLogStore) -> Vec<api_logs::ApiLog> {
        #[expect(clippy::expect_used, reason = "a healthy fixture store reads back")]
        let rows = list_api_logs(Some(store), 100, None, &api_logs::UsageFilter::default())
            .await
            .expect("the fixture store reads back");

        rows
    }

    /// Reads back the single recorded row of `store`.
    ///
    /// # Panics
    ///
    /// Panics if the read fails or the fixture did not record exactly one row.
    async fn recorded_row(store: &api_logs::ApiLogStore) -> api_logs::ApiLog {
        let rows = recorded_rows(store).await;

        assert_eq!(rows.len(), 1, "the fixture records exactly one row");

        #[expect(clippy::unwrap_used, reason = "the fixture wrote exactly one row")]
        rows.first().unwrap().clone()
    }

    /// Verifies the detached non-streaming finalization's abort arm (README §3 *ApiLog*): the
    /// handler departs — the oneshot receiver drops — before the upstream stream has ended, the
    /// stream still runs to completion, and the row finalizes `client_aborted` with `NULL`
    /// status, because no response was ever produced, carrying the usage the stream delivered
    /// before the abort.
    ///
    /// # Panics
    ///
    /// Panics if the finalization contract diverges.
    #[tokio::test]
    async fn finalizes_client_aborted_when_the_handler_departs_before_completion() {
        let (store, _guard) = test_store();
        let (events_sender, events) = event_channel();

        let (outcome_sender, outcome_receiver) = oneshot::channel();

        let task = aggregated_response_task::<ChatCompletionFormat>(
            events,
            "public-model".to_string(),
            Some(store.clone()),
            stamped_draft(),
            log_context(),
            outcome_sender,
        );

        // The handler departs mid-generation: hyper drops it together with the oneshot receiver.
        drop(outcome_receiver);

        // The detached task owns the upstream stream, so the stream runs to completion anyway.
        send_clean_events(&events_sender).await;
        drop(events_sender);

        task.await;

        let row = recorded_row(&store).await;

        assert_eq!(row.outcome, ApiLogOutcome::ClientAborted);
        assert_eq!(row.http_status, None);
        assert_eq!(row.input_tokens, Some(11));
        assert_eq!(row.output_tokens, Some(7));
    }

    /// Verifies the detached non-streaming finalization's success arm (README §3 *ApiLog*): the
    /// client stays, the rendered aggregated body is handed to the handler, and the row finalizes
    /// `completed` with the declared 200 and the stream's usage.
    ///
    /// # Panics
    ///
    /// Panics if the finalization contract diverges or the handed response is not the aggregated
    /// rendering.
    #[tokio::test]
    async fn finalizes_completed_and_hands_the_rendered_response_to_the_handler() {
        let (store, _guard) = test_store();
        let (events_sender, events) = event_channel();

        let (outcome_sender, outcome_receiver) = oneshot::channel();

        let task = aggregated_response_task::<ChatCompletionFormat>(
            events,
            "public-model".to_string(),
            Some(store.clone()),
            stamped_draft(),
            log_context(),
            outcome_sender,
        );

        send_clean_events(&events_sender).await;
        drop(events_sender);

        // The task future is driven to completion first: it hands the outcome into the oneshot,
        // whose single slot buffers it for the receive below.
        task.await;

        #[expect(
            clippy::expect_used,
            reason = "the task always hands its outcome, and the fixture aggregation succeeds"
        )]
        let response = outcome_receiver
            .await
            .expect("the task always hands its outcome to the handler")
            .expect("the fixture aggregation succeeds");

        assert_eq!(response.status(), StatusCode::OK);

        #[expect(clippy::expect_used, reason = "the fixture body reads and parses")]
        let body = serde_json::from_slice::<serde_json::Value>(
            &axum::body::to_bytes(response.into_body(), MAX_BODY_BYTES)
                .await
                .expect("the fixture body reads"),
        )
        .expect("the fixture body parses");

        assert_eq!(
            body.pointer("/model"),
            Some(&serde_json::json!("public-model"))
        );
        assert_eq!(
            body.pointer("/choices/0/message/content"),
            Some(&serde_json::json!("hello"))
        );

        let row = recorded_row(&store).await;

        assert_eq!(row.outcome, ApiLogOutcome::Completed);
        assert_eq!(row.http_status, Some(200));
        assert_eq!(row.input_tokens, Some(11));
        assert_eq!(row.output_tokens, Some(7));
    }

    /// Verifies the detached non-streaming finalization's upstream-failure arm (README §3
    /// *ApiLog*): a failed stream classifies the row through the shared error taxonomy
    /// regardless of the client — `upstream_error` with the transport-failure 502 both when the
    /// handler receives the failure for rendering and when the handler has already departed, the
    /// departure never upgrading to `client_aborted`.
    ///
    /// # Panics
    ///
    /// Panics if the finalization contract diverges.
    #[tokio::test]
    async fn finalizes_the_classified_upstream_failure_regardless_of_the_client() {
        // The client stays: the failure is handed to the handler for rendering.
        let (store, _guard) = test_store();
        let (events_sender, events) = event_channel();

        let (outcome_sender, outcome_receiver) = oneshot::channel();

        let task = aggregated_response_task::<ChatCompletionFormat>(
            events,
            "public-model".to_string(),
            Some(store.clone()),
            stamped_draft(),
            log_context(),
            outcome_sender,
        );

        send_failing_events(&events_sender).await;
        drop(events_sender);

        // The task future is driven to completion first: it hands the outcome into the oneshot,
        // whose single slot buffers it for the receive below.
        task.await;

        #[expect(clippy::expect_used, reason = "the task always hands its outcome")]
        let outcome = outcome_receiver
            .await
            .expect("the task always hands its outcome to the handler");

        assert!(
            matches!(outcome, Err(Error::UpstreamUnavailable { .. })),
            "the handler receives the stream failure for rendering"
        );

        let row = recorded_row(&store).await;

        assert_eq!(row.outcome, ApiLogOutcome::UpstreamError);
        assert_eq!(row.http_status, Some(502));

        // The client departed before the failure: the row still classifies the upstream failure —
        // never `client_aborted` — because the stream, not the client, is what failed.
        let (departed_store, _departed_guard) = test_store();
        let (departed_events_sender, departed_events) = event_channel();

        let (departed_outcome_sender, departed_outcome_receiver) = oneshot::channel();

        let departed_task = aggregated_response_task::<ChatCompletionFormat>(
            departed_events,
            "public-model".to_string(),
            Some(departed_store.clone()),
            stamped_draft(),
            log_context(),
            departed_outcome_sender,
        );

        drop(departed_outcome_receiver);

        send_failing_events(&departed_events_sender).await;
        drop(departed_events_sender);

        departed_task.await;

        let departed_row = recorded_row(&departed_store).await;

        assert_eq!(departed_row.outcome, ApiLogOutcome::UpstreamError);
        assert_eq!(departed_row.http_status, Some(502));
    }
}
