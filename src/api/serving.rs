//! The shared outward serving pipeline behind every `/api` format surface (README §2.2).
//!
//! One home for the machinery the four surfaces (`/openai-v1`, `/gemini-v1beta` GenerateContent,
//! `/gemini-v1beta` Interactions, `/anthropic-v1`) previously each carried a copy of: the
//! completion pipeline [`run_completion`], the SSE and aggregated response machinery with the
//! request-accounting finalization rules, the body-extractor rejection, the authentication
//! middleware, the 404/405 fallbacks, and the body-size constant. This duplication is README
//! §3's accounting invariant itself — one current owner (the account-attempt boundary,
//! the detached non-streaming aggregation task, or the SSE render task), the
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
    compaction::{
        accounting::CompactionLedger,
        executor::{self, MessagesOuterOutcome, ResponsesOuterOutcome},
        history::BoundaryPreparation,
    },
    context::Context,
    database::Database,
    format::{
        ClientFormat, ClientSseRenderer, CompactionBoundary, CompactionControl, NativeArtifact,
        ParsedRequest, RouteContext, aggregation_events,
        anthropic::messages::{CompactionOnlyUsage, render_compaction_only_response},
        openai::response::{parse_compact_request, render_trigger_stream_frames},
    },
    gateway::{self, CompactionDispatch, IrEventStream},
    ir::{Aggregator, IrAggregation, IrEvent, IrUsage},
    security,
    storage::{
        api_keys,
        api_logs::{self, ApiLogDraft},
        models::{self, EnabledModel},
    },
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{
        DefaultBodyLimit, FromRequestParts, OriginalUri, Request, State, rejection::BytesRejection,
    },
    http::{Method, StatusCode, header, request::Parts as RequestParts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::{convert::Infallible, io::Error as IoError, marker::PhantomData};
use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

/// Maximum accepted request body size, large enough for inline image data.
pub(super) const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

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
///
/// # Panics
///
/// Panics if positive decimal retry seconds cannot form a header, which their type prevents.
pub(super) fn error_response<S>(error: Error) -> Response
where
    S: Surface,
{
    let (status, envelope) = S::render_error(&error);

    let mut response = json_response(status, &envelope);
    if let Error::PoolQuotaExhausted {
        retry_after: Some(seconds),
        ..
    }
    | Error::BoundAccountQuotaExhausted {
        retry_after: Some(seconds),
        ..
    } = error
    {
        #[expect(
            clippy::expect_used,
            reason = "positive decimal seconds are a valid header value"
        )]
        let value = seconds
            .to_string()
            .parse()
            .expect("decimal Retry-After header");
        response
            .headers_mut()
            .insert(axum::http::header::RETRY_AFTER, value);
    }
    response
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

/// Composes one surface router's shared pipeline tail — the surface's 404/405 fallbacks
/// ([`fallback`], [`method_not_allowed`]), the authentication middleware ([`authenticate`]), and
/// the body limit ([`MAX_BODY_BYTES`]) — onto the surface's route set. Each surface's `router`
/// function owns only its route set and delegates this tail, so all three surfaces compose one
/// identical service tree.
///
/// The order is load-bearing, because an axum layer wraps everything registered before it:
///
/// - The authentication middleware wraps the routes and their fallbacks — the fallbacks are
///   registered first — so an unauthorized request is rejected with the surface's 401 before a 404
///   or 405 body is produced (README §2.2 item 4).
/// - The body limit is extractor-side configuration: the [`DefaultBodyLimit`] layer only plants the
///   limit into request extensions, and each handler's [`Bytes`] extractor enforces it, rendering
///   the surface's 413 ([`BodyError`]) at extraction time — after authentication, since the
///   handlers run within [`authenticate`]'s `next.run` (README §4: the 64 MiB outward body cap).
///
/// `database` feeds the authentication middleware its own state, so every request checks the
/// presented key against the live database rather than the router state.
pub(super) fn surface_router<S>(database: Database, routes: Router<Context>) -> Router<Context>
where
    // `'static` because the fallbacks and the authentication middleware are stored in the
    // router's service tree, whose handlers must be `'static`.
    S: Surface + 'static,
{
    routes
        .fallback(fallback::<S>)
        .method_not_allowed_fallback(method_not_allowed::<S>)
        .layer(middleware::from_fn_with_state(database, authenticate::<S>))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
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
/// [`gateway::execute`] for the dispatch mark. Exactly one current finalization owner then
/// records the row: the owned account-attempt boundary for a post-dispatch failure, the detached
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

    if parsed.compaction.is_none() {
        return run_plain_completion::<TFormat>(
            context,
            parsed,
            api_key_id,
            operation,
            endpoint_type,
        )
        .await;
    }

    let preparation = prepare_compaction_boundaries(&parsed, operation.as_str())?;

    let selection =
        match gateway::resolve_compaction_routing(context, &parsed, &preparation, api_key_id).await
        {
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

    let routing = selection.routing.clone();

    let log_context = format!(
        "{} for model `{}` via provider `{}`",
        operation.as_str(),
        routing.public_id,
        routing.provider.name
    );

    let public_id = routing.public_id.clone();

    let is_streaming = parsed.is_streaming;

    let should_include_usage = parsed.should_include_usage;

    // The accounting draft: every fact known before dispatch, built from the authenticated key,
    // the routing row, and the surface's endpoint type; `gateway::execute` stamps the dispatch
    // instant into it (README §3 *ApiLog*).
    let log = ApiLogDraft {
        started_at: None,
        api_key_id,
        provider_id: routing.provider.id,
        model_id: routing.model.id,
        model_route_id: routing.model_route_id,
        upstream_http_status: None,
        progress: crate::storage::api_logs::AttemptProgress::default(),
        endpoint_type,
        is_stream: is_streaming,
        billed_usage: None,
    };

    // A request carrying compaction controls or compaction state routes
    // through the compaction pipeline: restore, strategy, and the outcome
    // renderings README §6.8 pins. Every other request runs the ordinary
    // path unchanged.
    let request_model = parsed.ir.model.clone();

    let outcome = run_compaction_pipeline::<TFormat>(
        context,
        parsed,
        preparation,
        selection,
        log,
        log_context,
        is_streaming,
        should_include_usage,
        public_id,
        operation,
    )
    .await;

    if let Err(error) = &outcome {
        log_error(
            &format!("{} for model `{}`", operation.as_str(), request_model),
            error,
        );
    }

    outcome
}

/// Executes and renders an ordinary operation after quota-aware account selection.
/// Its final owner chooses outward error accounting; abandoned attempts are already closed.
///
/// # Errors
///
/// Returns parsing-independent selection or final account failures.
///
/// # Panics
///
/// Panics if the detached aggregation task violates its guaranteed outcome handoff.
async fn run_plain_completion<TFormat>(
    context: &Context,
    parsed: ParsedRequest,
    api_key_id: i64,
    operation: OperationKind,
    endpoint_type: ApiLogEndpointType,
) -> Result<Response, Error>
where
    TFormat: ClientFormat + 'static,
    TFormat::JsonResponse: Serialize,
{
    let public_id = parsed.ir.model.clone();
    let is_streaming = parsed.is_streaming;
    let should_include_usage = parsed.should_include_usage;
    let selected =
        match gateway::execute_selection(context, parsed, api_key_id, endpoint_type).await {
            Ok(selected) => selected,
            Err(error) => {
                let error = Error::from(error);
                log_error(
                    &format!("{} for model `{public_id}`", operation.as_str()),
                    &error,
                );
                return Err(error);
            }
        };
    let log_context = format!(
        "{} for model `{public_id}` via provider `{}`",
        operation.as_str(),
        selected.routing.provider.name
    );

    let outcome = async {
        let (events, log) = match selected.outcome {
            gateway::AttemptOutcome::Accepted { events, attempt } => (events, attempt.transfer()),
            gateway::AttemptOutcome::PredispatchFailure { error, attempt }
            | gateway::AttemptOutcome::Rejected { error, attempt }
            | gateway::AttemptOutcome::DispatchedFailure { error, attempt } => {
                let error = Error::from(error);
                let (outcome, status) = error.accounting();
                attempt.finalize(outcome, Some(status)).await;
                return Err(error);
            }
        };

        let affinity = selected.affinity;
        if is_streaming {
            Ok(sse_response(
                events,
                TFormat::sse_renderer(should_include_usage, public_id),
                context.api_log_store.clone(),
                log,
                affinity,
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
                affinity,
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

/// Authenticates the request's carriers before route selection, with no upstream or storage I/O.
/// The returned proof is retained through compatibility, restoration, and same-account fallback.
///
/// # Errors
///
/// Returns and logs a carrier authentication or visible-content binding failure.
fn prepare_compaction_boundaries(
    parsed: &ParsedRequest,
    operation: &str,
) -> Result<BoundaryPreparation, Error> {
    parsed
        .compaction
        .as_ref()
        .map(BoundaryPreparation::authenticate)
        .transpose()
        .map(|preparation| preparation.unwrap_or_default())
        .map_err(|error| {
            let error = Error::from(gateway::Error::from(error));
            log_error(&format!("{operation} request"), &error);
            error
        })
}

/// The kind of outward surface a compaction-carrying request renders through:
/// the surface decides the outcome renderings, never the restore itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompactionSurface {
    /// The OpenAI Responses surface (`/responses`).
    Responses,

    /// The Anthropic Messages surface (`/messages`).
    Messages,
}

/// Decides the compaction surface from the parsed request: the control names
/// it when one is present, and a control-less replay resolves through the
/// native artifacts it carries — a native Messages block rides the Messages
/// surface, everything else renders Responses-side (a pure gateway-carrier
/// replay is Ordinary on either).
fn compaction_surface(parsed: &crate::format::ParsedRequest) -> CompactionSurface {
    let Some(compaction) = parsed.compaction.as_ref() else {
        return CompactionSurface::Responses;
    };

    match &compaction.control {
        Some(
            CompactionControl::MessagesOnDemand { .. }
            | CompactionControl::MessagesThreshold { .. },
        ) => CompactionSurface::Messages,

        Some(
            CompactionControl::ResponsesAutomatic { .. }
            | CompactionControl::ResponsesStandalone
            | CompactionControl::ResponsesTrigger,
        ) => CompactionSurface::Responses,

        None => {
            let is_messages = compaction.boundaries.iter().any(|boundary| {
                matches!(
                    boundary,
                    CompactionBoundary::Native {
                        artifact: NativeArtifact::MessagesCompactionBlock { .. },
                        ..
                    }
                )
            });

            if is_messages {
                CompactionSurface::Messages
            } else {
                CompactionSurface::Responses
            }
        }
    }
}

/// The compaction pipeline for one parsed completion request: restore, resolve
/// strategy and native operations (README §6.8), and render the outcome —
/// the ordinary path for replays and below-threshold generations, the
/// codec-shaped compaction lifecycles for everything else.
///
/// Streaming note: a checkpoint-continue lifecycle renders the checkpoint from
/// the executor's artifact ahead of the live continuation events, so a
/// streaming client waits for the compaction to complete before its answer
/// streams — the gateway-emulated threshold contract (README §6.8).
/// Owns accounting until a compaction response task takes over, including handler cancellation.
struct CompactionLogGuard {
    /// Draft updated at every dispatch and usage observation.
    draft: ApiLogDraft,
    /// Store independent of the departing request.
    store: Option<api_logs::ApiLogStore>,
    /// Whether a response or finalization task now owns the row.
    has_transferred: bool,
}

impl Drop for CompactionLogGuard {
    fn drop(&mut self) {
        if !self.has_transferred && self.draft.started_at.is_some() {
            let draft = self.draft.clone();
            let store = self.store.clone();
            tokio::spawn(async move {
                api_logs::finalize_api_log(
                    store.as_ref(),
                    &draft,
                    ApiLogOutcome::ClientAborted,
                    None,
                    None,
                )
                .await;
            });
        }
    }
}

/// Finalizes a compaction error independently of a canceled handler.
async fn finalize_compaction_failure(context: &Context, log: &ApiLogDraft, error: &Error) {
    let (outcome, status) = error.accounting();
    let draft = log.clone();
    let store = context.api_log_store.clone();
    let task = tokio::spawn(async move {
        api_logs::finalize_api_log(store.as_ref(), &draft, outcome, Some(status), None).await;
    });
    let _ = task.await;
}

/// Coordinates compaction accounting until the response takes ownership of it.
#[allow(clippy::too_many_arguments)]
async fn run_compaction_pipeline<TFormat>(
    context: &Context,
    parsed: ParsedRequest,
    preparation: BoundaryPreparation,
    mut selection: gateway::CompactionSelection,
    log: ApiLogDraft,
    mut log_context: String,
    is_streaming: bool,
    should_include_usage: bool,
    public_id: String,
    operation: OperationKind,
) -> Result<Response, Error>
where
    TFormat: ClientFormat + 'static,
    TFormat::JsonResponse: Serialize,
{
    let mut guard = CompactionLogGuard {
        draft: log,
        store: context.api_log_store.clone(),
        has_transferred: false,
    };
    let outcome = loop {
        let routing = selection.routing.clone();
        let outcome = run_compaction_pipeline_inner::<TFormat>(
            context,
            parsed.clone(),
            &preparation,
            &mut selection,
            routing,
            &mut guard.draft,
            log_context.clone(),
            is_streaming,
            should_include_usage,
            public_id.clone(),
            operation,
            &mut guard.has_transferred,
        )
        .await;
        if matches!(outcome, Err(Error::QuotaRejected { .. })) {
            match selection
                .advance_after_quota_rejection(context, &mut guard.draft)
                .await
            {
                Ok(true) => {
                    log_context = format!(
                        "{} for model `{}` via provider `{}`",
                        operation.as_str(),
                        selection.routing.public_id,
                        selection.routing.provider.name,
                    );
                    continue;
                }
                Ok(false) => {}
                Err(error) => break Err(error.into()),
            }
        }
        break outcome;
    };
    if !guard.has_transferred {
        guard.has_transferred = true;
        if let Err(error) = &outcome {
            finalize_compaction_failure(context, &guard.draft, error).await;
        }
    }
    outcome
}

/// Resolves and renders one compaction attempt, with errors owned by the outer accounting guard.
#[allow(clippy::too_many_arguments)]
async fn run_compaction_pipeline_inner<TFormat>(
    context: &Context,
    parsed: crate::format::ParsedRequest,
    preparation: &BoundaryPreparation,
    selection: &mut gateway::CompactionSelection,
    routing: crate::provider::ModelRouting,
    log: &mut ApiLogDraft,
    log_context: String,
    is_streaming: bool,
    should_include_usage: bool,
    public_id: String,
    operation: OperationKind,
    has_transferred: &mut bool,
) -> Result<Response, Error>
where
    TFormat: ClientFormat + 'static,
    TFormat::JsonResponse: Serialize,
{
    let surface = compaction_surface(&parsed);

    // The ledger's billed totals stamp the accounting row for every outcome
    // that carries one; the executor has already recorded each dispatch into
    // it (README §6.8).
    let stamp_billed = |log: &mut ApiLogDraft, ledger: Option<&CompactionLedger>| {
        if let Some(ledger) = ledger {
            log.stamp_billed_usage(ledger.reported_billed_totals());
        }
    };

    if surface == CompactionSurface::Messages {
        let outcome = executor::execute_messages_request(
            context,
            &routing,
            &parsed,
            preparation,
            log,
            Some(&mut selection.ownership),
        )
        .await?;

        match outcome {
            MessagesOuterOutcome::Ordinary {
                ir,
                threshold_edits,
                overlay_headers,
            } => {
                let dispatch = CompactionDispatch {
                    threshold_edits,
                    overlay_headers,
                    ..CompactionDispatch::default()
                };

                ordinary_completion::<TFormat>(
                    context,
                    routing,
                    ir,
                    log,
                    log_context,
                    is_streaming,
                    should_include_usage,
                    public_id.clone(),
                    dispatch,
                    parsed.clone(),
                    preparation,
                    selection,
                    operation,
                    has_transferred,
                )
                .await
            }

            MessagesOuterOutcome::CompactionOnly {
                block,
                artifact_id,
                iterations,
                ledger,
            } => {
                stamp_billed(log, Some(&ledger));

                let affinity = selection.accepted().map(|accepted| {
                    accepted.completed_prefix(
                        crate::format::anthropic::messages::completed_compaction_anchors(
                            &block,
                            artifact_id.as_deref(),
                        ),
                    )
                });
                compaction_only_messages_response(
                    context,
                    &public_id,
                    &block,
                    &iterations,
                    is_streaming,
                    parsed.compaction.as_ref().is_some_and(|compaction| {
                        matches!(
                            compaction.control,
                            Some(CompactionControl::MessagesThreshold { .. })
                        )
                    }),
                    log.clone(),
                    affinity,
                )
            }

            MessagesOuterOutcome::NativeNoSummary {
                aggregation,
                ledger,
            } => {
                stamp_billed(log, Some(&ledger));

                aggregated_artifact_response(
                    context,
                    &public_id,
                    &aggregation,
                    is_streaming,
                    should_include_usage,
                    log.clone(),
                    selection.accepted(),
                )
            }

            MessagesOuterOutcome::ContinueWithCheckpoint {
                prefix_blocks,
                artifact_id,
                ir,
                overlay_headers,
                iterations,
                ledger,
            } => {
                stamp_billed(log, Some(&ledger));

                checkpoint_completion::<TFormat>(
                    context,
                    routing,
                    ir,
                    log,
                    log_context,
                    is_streaming,
                    should_include_usage,
                    public_id,
                    Vec::new(),
                    prefix_blocks,
                    iterations,
                    overlay_headers,
                    has_transferred,
                    selection,
                    artifact_id,
                )
                .await
            }
        }
    } else {
        let outcome = executor::execute_responses_request(
            context,
            &routing,
            &parsed,
            preparation,
            log,
            Some(&mut selection.ownership),
        )
        .await?;

        match outcome {
            ResponsesOuterOutcome::Ordinary {
                ir,
                context_management,
                overlay_headers,
            } => {
                let dispatch = CompactionDispatch {
                    context_management,
                    overlay_headers,
                    ..CompactionDispatch::default()
                };

                ordinary_completion::<TFormat>(
                    context,
                    routing,
                    ir,
                    log,
                    log_context,
                    is_streaming,
                    should_include_usage,
                    public_id.clone(),
                    dispatch,
                    parsed.clone(),
                    preparation,
                    selection,
                    operation,
                    has_transferred,
                )
                .await
            }

            ResponsesOuterOutcome::CompactionOnly {
                items,
                artifact_id,
                usage,
                ledger,
            } => {
                stamp_billed(log, Some(&ledger));

                let response = compaction_only_responses_response(
                    context,
                    &public_id,
                    &items,
                    usage,
                    is_streaming,
                    log.clone(),
                )?;
                selection.register_gateway_window(&artifact_id, &items);
                Ok(response)
            }

            ResponsesOuterOutcome::NativeLifecycle {
                aggregation,
                ledger,
            } => {
                stamp_billed(log, Some(&ledger));
                let affinity = selection.accepted();
                aggregated_artifact_response(
                    context,
                    &public_id,
                    &aggregation,
                    is_streaming,
                    should_include_usage,
                    log.clone(),
                    affinity,
                )
            }

            ResponsesOuterOutcome::ContinueWithCheckpoint {
                prefix_items,
                artifact_id,
                ir,
                overlay_headers,
                ledger,
            } => {
                stamp_billed(log, Some(&ledger));

                checkpoint_completion::<TFormat>(
                    context,
                    routing,
                    ir,
                    log,
                    log_context,
                    is_streaming,
                    should_include_usage,
                    public_id,
                    prefix_items,
                    Vec::new(),
                    Vec::new(),
                    overlay_headers,
                    has_transferred,
                    selection,
                    artifact_id,
                )
                .await
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// The ordinary continuation of a compaction pipeline outcome: the prepared
/// request runs the existing execute-and-render path, carrying the compaction
/// dispatch extension the executor resolved (passthrough controls, replay
/// overlays — empty for a plain replay).
async fn ordinary_completion<TFormat>(
    context: &Context,
    routing: crate::provider::ModelRouting,
    ir: crate::ir::IrRequest,
    log: &mut ApiLogDraft,
    log_context: String,
    is_streaming: bool,
    should_include_usage: bool,
    public_id: String,
    dispatch: CompactionDispatch,
    fallback_request: ParsedRequest,
    preparation: &BoundaryPreparation,
    selection: &mut gateway::CompactionSelection,
    operation: OperationKind,
    has_transferred: &mut bool,
) -> Result<Response, Error>
where
    TFormat: ClientFormat + 'static,
    TFormat::JsonResponse: Serialize,
{
    let events = gateway::execute(
        context,
        routing.clone(),
        ir,
        is_streaming,
        log,
        Some(&dispatch),
        Some(&mut selection.ownership),
    )
    .await;
    let events = match events {
        Ok(events) => events,
        Err(gateway::Error::Upstream {
            status,
            detail: message,
        }) => {
            let section = routing.provider.definition.payload.compaction.as_ref();
            let classifier = if dispatch.context_management.is_some() {
                section
                    .and_then(|section| section.mechanisms.responses_automatic_compaction.as_ref())
                    .and_then(|declaration| declaration.unsupported.as_ref())
            } else if dispatch.threshold_edits.is_some() {
                section
                    .and_then(|section| section.mechanisms.messages_threshold_compaction.as_ref())
                    .and_then(|declaration| declaration.unsupported.as_ref())
            } else {
                None
            };
            if crate::compaction::native::classify_failure(classifier, status, &message)
                .await
                .is_some()
            {
                let mut fallback_routing = routing;
                if let Some(section) = &mut fallback_routing.provider.definition.payload.compaction
                {
                    section.strategy =
                        crate::provider::definition::compaction::CompactionStrategy::GatewaySummary;
                }
                return Box::pin(run_compaction_pipeline_inner::<TFormat>(
                    context,
                    fallback_request,
                    preparation,
                    selection,
                    fallback_routing,
                    log,
                    log_context,
                    is_streaming,
                    should_include_usage,
                    public_id,
                    operation,
                    has_transferred,
                ))
                .await;
            }
            return Err(Error::Upstream { status, message });
        }
        Err(error) => return Err(error.into()),
    };

    *has_transferred = true;
    if is_streaming {
        let is_threshold = fallback_request
            .compaction
            .as_ref()
            .is_some_and(|compaction| {
                matches!(
                    compaction.control,
                    Some(CompactionControl::MessagesThreshold { .. })
                )
            });
        let renderer: Box<dyn ClientSseRenderer + Send> = if is_threshold {
            Box::new(
                crate::format::anthropic::messages::SseRenderer::new(public_id)
                    .with_threshold_compaction(true),
            )
        } else {
            TFormat::sse_renderer(should_include_usage, public_id)
        };
        Ok(sse_response(
            events,
            renderer,
            context.api_log_store.clone(),
            log.clone(),
            selection.accepted(),
        ))
    } else {
        detached_aggregation::<TFormat>(
            events,
            public_id,
            context.api_log_store.clone(),
            log.clone(),
            log_context,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            selection.accepted(),
        )
        .await
    }
}

#[allow(clippy::too_many_arguments)]
/// The checkpoint-continue outcome: the continuation runs the existing
/// execute-and-render path while the executor's checkpoint renders ahead of
/// it — prefix items on the Responses surface, prefix blocks on Messages,
/// beside the lifecycle's vendor-shaped iteration entries on the Messages
/// surface (README §6.8).
///
/// # Errors
///
/// Returns the selected account's continuation dispatch or response failure.
///
/// # Panics
///
/// Panics if the executor's sealed single checkpoint lacks its replayable wire fields.
async fn checkpoint_completion<TFormat>(
    context: &Context,
    routing: crate::provider::ModelRouting,
    ir: crate::ir::IrRequest,
    log: &mut ApiLogDraft,
    log_context: String,
    is_streaming: bool,
    should_include_usage: bool,
    public_id: String,
    prefix_items: Vec<serde_json::Value>,
    prefix_blocks: Vec<serde_json::Value>,
    iterations: Vec<serde_json::Value>,
    overlay_headers: Vec<crate::provider::definition::template::ParamRow>,
    has_transferred: &mut bool,
    selection: &mut gateway::CompactionSelection,
    artifact_id: String,
) -> Result<Response, Error>
where
    TFormat: ClientFormat + 'static,
    TFormat::JsonResponse: Serialize,
{
    let dispatch = CompactionDispatch {
        overlay_headers,
        ..CompactionDispatch::default()
    };
    let events = gateway::execute(
        context,
        routing,
        ir,
        is_streaming,
        log,
        Some(&dispatch),
        Some(&mut selection.ownership),
    )
    .await?;

    let affinity = selection.accepted().map(|accepted| {
        let (projector, anchors) = if prefix_blocks.is_empty() {
            #[expect(
                clippy::expect_used,
                reason = "the executor emits a sealed replayable checkpoint"
            )]
            let projector =
                crate::format::openai::response::gateway_checkpoint_projection(&prefix_items)
                    .expect("a prepared Responses checkpoint has a canonical projection");
            let mut anchors = projector.completed_anchors();
            anchors.push(crate::affinity::Anchor::Artifact(
                projector.gateway_artifact(&artifact_id),
            ));
            (projector, anchors)
        } else {
            #[expect(
                clippy::expect_used,
                reason = "the executor emits one sealed Messages checkpoint"
            )]
            let block = prefix_blocks
                .first()
                .expect("the checkpoint prefix contains its block");
            #[expect(
                clippy::expect_used,
                reason = "the sealed block has accepted replay fields"
            )]
            crate::format::anthropic::messages::completed_compaction_projection(
                block,
                Some(&artifact_id),
            )
            .expect("a prepared Messages checkpoint has a canonical projection")
        };
        accepted.completed_checkpoint(projector, anchors)
    });

    *has_transferred = true;
    if is_streaming {
        let renderer = if prefix_blocks.is_empty() {
            openai_sse_renderer_with_items(should_include_usage, public_id, prefix_items)
        } else {
            anthropic_sse_renderer_with_blocks(public_id, prefix_blocks, iterations)
        };

        Ok(sse_response(
            events,
            renderer,
            context.api_log_store.clone(),
            log.clone(),
            affinity,
        ))
    } else {
        detached_aggregation::<TFormat>(
            events,
            public_id,
            context.api_log_store.clone(),
            log.clone(),
            log_context,
            prefix_items,
            prefix_blocks,
            iterations,
            affinity,
        )
        .await
    }
}

/// Boxes the Responses streaming renderer carrying checkpoint prefix items.
fn openai_sse_renderer_with_items(
    should_include_usage: bool,
    public_id: String,
    prefix_items: Vec<serde_json::Value>,
) -> Box<dyn ClientSseRenderer + Send> {
    let _ = should_include_usage;

    Box::new(
        crate::format::openai::response::SseRenderer::new(public_id.clone())
            .with_prefix_items(prefix_items),
    )
}

/// Boxes the Messages streaming renderer carrying checkpoint prefix blocks and
/// the lifecycle's iteration entries.
fn anthropic_sse_renderer_with_blocks(
    public_id: String,
    prefix_blocks: Vec<serde_json::Value>,
    iterations: Vec<serde_json::Value>,
) -> Box<dyn ClientSseRenderer + Send> {
    Box::new(
        crate::format::anthropic::messages::SseRenderer::with_compaction_prefix(
            public_id,
            prefix_blocks,
            None,
        )
        .with_usage_iterations(iterations),
    )
}

/// Renders one fully-buffered compaction artifact as the request's non-streaming
/// body, finalizing the row as the successful hand-off it is.
/// Renders one fully-buffered compaction artifact as the request's
/// non-streaming body, finalizing the row as the successful hand-off it is.
///
/// # Panics
///
/// Panics if the prerendered SSE response cannot build — impossible for a
/// static status, header, and string body.
fn finalize_buffered<T: Serialize>(context: &Context, log: &ApiLogDraft, body: &T) -> Response {
    // The body is complete before this call, so the row's outcome is the
    // successful hand-off; a client departing after the hand-off is the
    // documented residual edge (README §3 *ApiLog*).
    let store = context.api_log_store.clone();

    let response = json_response(axum::http::StatusCode::OK, body);

    let log = log.clone();

    tokio::spawn(async move {
        api_logs::finalize_api_log(
            store.as_ref(),
            &log,
            ApiLogOutcome::Completed,
            Some(i64::from(axum::http::StatusCode::OK.as_u16())),
            None,
        )
        .await;
    });

    response
}

/// Renders the Responses compact-only outcome: the synthesized trigger
/// lifecycle — streaming callers receive the complete frame sequence, JSON
/// callers the same artifact as the aggregated body.
fn compaction_only_responses_response(
    context: &Context,
    public_id: &str,
    items: &[serde_json::Value],
    usage: IrUsage,
    is_streaming: bool,
    log: ApiLogDraft,
) -> Result<Response, Error> {
    if is_streaming {
        let frames = render_trigger_stream_frames(public_id.to_string(), items.to_vec(), usage);

        prerendered_sse_response(frames, context.api_log_store.clone(), log)
    } else {
        let aggregation = IrAggregation {
            usage: Some(usage),
            ..IrAggregation::default()
        };

        let body = crate::format::openai::response::Format::render_json_response_with_prefix(
            public_id,
            &aggregation,
            items,
            &[],
        );

        Ok(finalize_buffered(context, &log, &body))
    }
}

/// Renders the Messages compact-only outcome: one compaction block with
/// `stop_reason: "compaction"` and the iteration usage (README §6.8).
#[expect(
    clippy::too_many_arguments,
    reason = "delivery owns the prepared account binding"
)]
fn compaction_only_messages_response(
    context: &Context,
    public_id: &str,
    block: &serde_json::Value,
    iterations: &[serde_json::Value],
    is_streaming: bool,
    is_threshold_compaction: bool,
    log: ApiLogDraft,
    mut affinity: Option<crate::affinity::Accepted>,
) -> Result<Response, Error> {
    if is_streaming {
        let renderer = crate::format::anthropic::messages::SseRenderer::with_compaction_prefix(
            public_id.to_string(),
            vec![block.clone()],
            Some(String::from("compaction")),
        )
        .with_usage_iterations(iterations.to_vec())
        .with_threshold_compaction(is_threshold_compaction);

        let events = futures::stream::iter(vec![
            Ok::<IrEvent, gateway::Error>(IrEvent::MessageStart {
                id: public_id.to_string(),
                model: public_id.to_string(),
            }),
            Ok(IrEvent::Done),
        ]);

        Ok(sse_response(
            Box::pin(events),
            Box::new(renderer),
            context.api_log_store.clone(),
            log,
            affinity,
        ))
    } else {
        let usage = CompactionOnlyUsage {
            iterations: iterations.to_vec(),
            input_tokens: 0,
            output_tokens: 0,
            cache_write_tokens: None,
            cached_tokens: None,
        };

        let body = render_compaction_only_response(public_id, block.clone(), &usage);

        if let Some(affinity) = affinity.as_mut() {
            affinity.complete_artifacts();
        }
        Ok(finalize_buffered(context, &log, &body))
    }
}

/// Renders a lifecycle the executor validated as one complete internal
/// artifact — a native trigger passthrough or a native no-summary outcome —
/// from its aggregation: the same body streaming and JSON callers see.
fn aggregated_artifact_response(
    context: &Context,
    public_id: &str,
    aggregation: &IrAggregation,
    is_streaming: bool,
    should_include_usage: bool,
    mut log: ApiLogDraft,
    mut affinity: Option<crate::affinity::Accepted>,
) -> Result<Response, Error> {
    let _ = should_include_usage;

    if is_streaming {
        // This same native observation is already present in the ledger; the streamed
        // events own its final observation instead, so it must not be added twice.
        log.billed_usage = None;
        let events = aggregation_events(aggregation, public_id);

        let renderer = if log.endpoint_type == ApiLogEndpointType::AnthropicMessages {
            Box::new(crate::format::anthropic::messages::SseRenderer::new(
                public_id.to_string(),
            )) as Box<dyn ClientSseRenderer + Send>
        } else {
            Box::new(crate::format::openai::response::SseRenderer::new(
                public_id.to_string(),
            )) as Box<dyn ClientSseRenderer + Send>
        };

        let stream: IrEventStream = Box::pin(futures::stream::iter(
            events.into_iter().map(Ok::<IrEvent, gateway::Error>),
        ));

        Ok(sse_response(
            stream,
            renderer,
            context.api_log_store.clone(),
            log,
            affinity,
        ))
    } else {
        // A validated single-artifact lifecycle carries no checkpoint prefix:
        // the plain aggregated render keeps its native semantics — usage
        // without gateway-fabricated iteration entries.
        if log.endpoint_type == ApiLogEndpointType::AnthropicMessages {
            let body = crate::format::anthropic::messages::Format::render_json_response(
                public_id,
                aggregation,
            );
            if let Some(affinity) = &mut affinity {
                affinity.complete_json::<crate::format::anthropic::messages::Format>(&body);
            }
            Ok(finalize_buffered(context, &log, &body))
        } else {
            let body = crate::format::openai::response::Format::render_json_response(
                public_id,
                aggregation,
            );
            if let Some(affinity) = &mut affinity {
                affinity.complete_json::<crate::format::openai::response::Format>(&body);
            }
            Ok(finalize_buffered(context, &log, &body))
        }
    }
}

/// Returns the pre-rendered SSE frames as the response body, finalizing the
/// row for the completed artifact.
///
/// # Panics
///
/// Panics if the static content-type header value fails to parse —
/// impossible for a fixed literal.
fn prerendered_sse_response(
    frames: Vec<String>,
    api_log_store: Option<api_logs::ApiLogStore>,
    log: ApiLogDraft,
) -> Result<Response, Error> {
    let body = frames.concat();

    tokio::spawn(async move {
        api_logs::finalize_api_log(
            api_log_store.as_ref(),
            &log,
            ApiLogOutcome::Completed,
            Some(i64::from(axum::http::StatusCode::OK.as_u16())),
            None,
        )
        .await;
    });

    let mut response = axum::response::Response::new(axum::body::Body::from(body));

    *response.status_mut() = axum::http::StatusCode::OK;

    #[expect(
        clippy::expect_used,
        reason = "the static content-type value is a valid header value"
    )]
    let content_type: axum::http::HeaderValue = "text/event-stream"
        .parse()
        .expect("the static content-type parses");

    response
        .headers_mut()
        .insert(axum::http::header::CONTENT_TYPE, content_type);

    Ok(response)
}

#[allow(clippy::too_many_arguments)]
/// The detached non-streaming aggregation with checkpoint prefix state and the
/// lifecycle's iteration entries, rendered through
/// [`ClientFormat::render_json_response_with_prefix`].
///
/// # Panics
///
/// Panics if the aggregation task never hands its outcome to the handler —
/// an invariant violation, since the task owns exactly one send.
async fn detached_aggregation<TFormat>(
    events: IrEventStream,
    public_id: String,
    api_log_store: Option<api_logs::ApiLogStore>,
    log: ApiLogDraft,
    log_context: String,
    prefix_items: Vec<serde_json::Value>,
    prefix_blocks: Vec<serde_json::Value>,
    iterations: Vec<serde_json::Value>,
    affinity: Option<crate::affinity::Accepted>,
) -> Result<Response, Error>
where
    TFormat: ClientFormat + 'static,
    TFormat::JsonResponse: Serialize,
{
    let (outcome_sender, outcome_receiver) = tokio::sync::oneshot::channel();

    let aggregation = tokio::spawn(aggregated_with_prefix_task::<TFormat>(
        events,
        public_id,
        api_log_store,
        log,
        log_context,
        outcome_sender,
        prefix_items,
        prefix_blocks,
        iterations,
        affinity,
    ));

    drop(aggregation);

    #[expect(
        clippy::expect_used,
        reason = "the aggregation task always hands its outcome to the handler exactly once; \
                  a missing hand-off is an invariant violation that fails fast"
    )]
    outcome_receiver
        .await
        .expect("the aggregation task always hands its outcome to the handler")
}

/// The checkpoint variant of the detached aggregation task: the rendered body
/// opens with the compaction checkpoint state.
#[allow(clippy::too_many_arguments)]
/// The task body of [`detached_aggregation`]: consumes the stream, renders
/// the prefixed body, and finalizes the accounting row by the hand-off
/// outcome (README §3 *ApiLog*).
///
/// # Panics
///
/// Panics if serializing the rendered body fails — impossible for concrete
/// first-party response types.
async fn aggregated_with_prefix_task<TFormat>(
    events: IrEventStream,
    public_id: String,
    api_log_store: Option<api_logs::ApiLogStore>,
    log: ApiLogDraft,
    log_context: String,
    outcome_sender: tokio::sync::oneshot::Sender<Result<Response, Error>>,
    prefix_items: Vec<serde_json::Value>,
    prefix_blocks: Vec<serde_json::Value>,
    iterations: Vec<serde_json::Value>,
    mut affinity: Option<crate::affinity::Accepted>,
) where
    TFormat: ClientFormat,
    TFormat::JsonResponse: Serialize,
{
    let aggregation = aggregated_response(events, |aggregation| {
        // The surface decides which checkpoint state rides the render's
        // prefix: the executor fills exactly one of the two vectors.
        let prefix = if prefix_items.is_empty() {
            &prefix_blocks
        } else {
            &prefix_items
        };

        let body =
            TFormat::render_json_response_with_prefix(&public_id, aggregation, prefix, &iterations);

        if !outcome_sender.is_closed()
            && let Some(affinity) = affinity.as_mut()
        {
            affinity.complete_json::<TFormat>(&body);
        }
        body
    })
    .await;

    match aggregation {
        Ok((response, usage)) => {
            if outcome_sender.send(Ok(response)).is_ok() {
                api_logs::finalize_api_log(
                    api_log_store.as_ref(),
                    &log,
                    ApiLogOutcome::Completed,
                    Some(i64::from(axum::http::StatusCode::OK.as_u16())),
                    usage,
                )
                .await;
            } else {
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

        Err((error, usage)) => {
            let (outcome, http_status) = error.accounting();

            api_logs::finalize_api_log(
                api_log_store.as_ref(),
                &log,
                outcome,
                Some(http_status),
                usage,
            )
            .await;

            if let Err(Err(error)) = outcome_sender.send(Err(error)) {
                log_error(&log_context, &error);
            }
        }
    }
}

/// Routes one Responses standalone compact request (README §6.8): parse
/// the compact body, resolve routing, and hand the operation to the compaction
/// executor. The compact endpoint is Responses-only, so this entry — unlike
/// [`run_completion`] — is not format-generic.
///
/// The request is always non-streaming JSON, and a success body renders the
/// `response.compaction` object the codec owns. The accounting draft is built
/// from the routing facts but never stamped until a dispatch occurs: while the
/// executor placeholder answers, the request fails before dispatch and records
/// no row (README §3 *ApiLog*, README §6.8).
///
/// Every failure that reaches this boundary is logged exactly once here, with
/// the stage's context: a parse rejection names only the endpoint, a routing
/// rejection names the requested model, and the executor failure names the
/// resolved provider.
///
/// # Errors
///
/// Returns [`Error`] for request, routing, and executor failures.
pub(super) async fn run_compact(
    context: &Context,
    body: &[u8],
    api_key_id: i64,
) -> Result<Response, Error> {
    let parsed = match parse_compact_request(body) {
        Ok(parsed) => parsed,

        Err(error) => {
            let error = Error::from(error);

            log_error("compact request", &error);

            return Err(error);
        }
    };

    let preparation = prepare_compaction_boundaries(&parsed, "compact")?;

    let mut selection =
        match gateway::resolve_compaction_routing(context, &parsed, &preparation, api_key_id).await
        {
            Ok(routing) => routing,

            Err(error) => {
                let error = Error::from(error);

                log_error(&format!("compact for model `{}`", parsed.ir.model), &error);

                return Err(error);
            }
        };

    let routing = selection.routing.clone();

    // The accounting draft: every fact known before dispatch. The executor
    // stamps the dispatch instant at each internal send and the billed totals
    // on success, so a row exists exactly when a dispatch occurred and the
    // finalized row carries the participating calls' totals (README §3
    // *ApiLog*).
    let log = ApiLogDraft {
        started_at: None,
        api_key_id,
        provider_id: routing.provider.id,
        model_id: routing.model.id,
        model_route_id: routing.model_route_id,
        upstream_http_status: None,
        progress: crate::storage::api_logs::AttemptProgress::default(),
        endpoint_type: ApiLogEndpointType::OpenaiResponsesCompact,
        is_stream: false,
        billed_usage: None,
    };

    let mut guard = CompactionLogGuard {
        draft: log,
        store: context.api_log_store.clone(),
        has_transferred: false,
    };
    let outcome = loop {
        let outcome = executor::execute_responses_compact(
            context,
            &selection.routing,
            &parsed,
            &preparation,
            &mut guard.draft,
            Some(&mut selection.ownership),
        )
        .await;
        if matches!(outcome, Err(Error::QuotaRejected { .. })) {
            match selection
                .advance_after_quota_rejection(context, &mut guard.draft)
                .await
            {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => break Err(error.into()),
            }
        }
        break outcome;
    };
    guard.has_transferred = true;
    match outcome {
        Ok(body) => {
            selection.register_compacted(&body);
            Ok(finalize_buffered(context, &guard.draft, &body))
        }

        Err(error) => {
            finalize_compaction_failure(context, &guard.draft, &error).await;
            log_error(
                &format!(
                    "compact for model `{}` via provider `{}`",
                    selection.routing.public_id, selection.routing.provider.name
                ),
                &error,
            );

            Err(error)
        }
    }
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
///   regardless of the client — the upstream failure, not the departure, is what the row records —
///   carrying whatever usage the stream delivered before it failed (`NULL` columns when it
///   delivered none, README §6.2's last-observation rule).
async fn aggregated_response_task<TFormat>(
    events: IrEventStream,
    public_id: String,
    api_log_store: Option<api_logs::ApiLogStore>,
    log: ApiLogDraft,
    log_context: String,
    outcome_sender: tokio::sync::oneshot::Sender<Result<Response, Error>>,
    mut affinity: Option<crate::affinity::Accepted>,
) where
    TFormat: ClientFormat,
    TFormat::JsonResponse: Serialize,
{
    let aggregation = aggregated_response(events, |aggregation| {
        let response = TFormat::render_json_response(&public_id, aggregation);
        // Known handler departure creates no completed output. A later failed terminal handoff
        // leaves the already-created account hint harmless, without claiming client receipt.
        if !outcome_sender.is_closed()
            && let Some(affinity) = affinity.as_mut()
        {
            affinity.complete_json::<TFormat>(&response);
        }
        response
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

        Err((error, usage)) => {
            // An upstream failure records its classification regardless of the client, carrying
            // the usage the stream delivered before it failed — `None` when it delivered none.
            let (outcome, http_status) = error.accounting();

            api_logs::finalize_api_log(
                api_log_store.as_ref(),
                &log,
                outcome,
                Some(http_status),
                usage,
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

/// Renders the outward `GET /models` listing shared by every surface (README §6.3/§6.5): it
/// lists the effectively enabled models with their effective capability declarations over the
/// shared pipeline's database and maps them onto the surface's native page shape, or converts
/// the storage failure and renders it through the surface's error envelope under the shared
/// `"model listing"` log target. Each surface supplies only its success-shape mapping.
///
/// `render_page` receives the listed models by value, ordered by public ID, and returns the
/// surface's typed success body, which [`json_response`] serializes; each entry's capability
/// values are already the effective `override ?? base` composition, with `None` (unknown) left
/// for the surface to omit (README §6.5).
pub(super) async fn model_list_response<S, T, F>(database: &Database, render_page: F) -> Response
where
    S: Surface,
    F: FnOnce(Vec<EnabledModel>) -> T,
    T: Serialize,
{
    match models::list_enabled_models_with_owner(database).await {
        Ok(listed) => json_response(StatusCode::OK, &render_page(listed)),

        Err(error) => {
            let error = Error::from(error);

            log_error("model listing", &error);

            error_response::<S>(error)
        }
    }
}

/// Wraps an IR event stream into a live `text/event-stream` response, carrying the request's
/// accounting draft and api-log store handle into the render task: that task is the streaming
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
    mut affinity: Option<crate::affinity::Accepted>,
) -> Response {
    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<Bytes, IoError>>(gateway::EVENT_CHANNEL_BUFFER);

    tokio::spawn(async move {
        let mut events = events;
        // The codec receives only accepted client history; the task retains account ownership.
        if let Some(affinity) = affinity.as_ref() {
            affinity.begin_projection(renderer.as_mut());
        }

        // Whether any frame reached the client's channel; the first one declares HTTP 200.
        let mut is_status_declared = false;

        // The last usage the stream reported, recorded best-effort at finalization.
        let mut usage: Option<IrUsage> = None;
        let mut iteration_usage: Option<IrUsage> = None;
        // A codec can render no additional terminal frame when usage is absent; EOF completes it.
        let mut has_completed_output = false;

        while let Some(item) = events.next().await {
            match item {
                Ok(event) => {
                    if let IrEvent::CompactionMetadata { iterations, .. } = &event {
                        iteration_usage =
                            crate::compaction::accounting::reported_iteration_totals(iterations);
                    }
                    if let IrEvent::Usage(reported) = &event {
                        usage = Some(*reported);
                    }

                    has_completed_output |= matches!(event, IrEvent::Done);
                    let frames = renderer.render(&event);
                    let completed_artifact = renderer.take_completed_artifact();
                    let terminal_index = if matches!(event, IrEvent::Done) {
                        frames.len().checked_sub(1)
                    } else {
                        None
                    };
                    let prefix_boundary = renderer.take_prefix_boundary();
                    let artifact_terminal_index = affinity
                        .as_ref()
                        .and_then(|accepted| accepted.prepared_prefix_boundary(prefix_boundary));
                    for (index, frame) in frames.into_iter().enumerate() {
                        if completed_artifact
                            .as_ref()
                            .is_some_and(|artifact| artifact.terminal_index == index)
                            && !sender.is_closed()
                            && let Some(affinity) = affinity.as_ref()
                            && let Some(artifact) = &completed_artifact
                        {
                            affinity.register_released_artifact(&artifact.anchors);
                        }
                        // The validated native item or prepared single-block prefix is complete.
                        // Publish its digest immediately before that item's
                        // final emitted frame, after its earlier frames
                        // passed the channel. A later answer failure does
                        // not erase a completed released checkpoint.
                        if Some(index) == artifact_terminal_index
                            && !sender.is_closed()
                            && let Some(affinity) = affinity.as_mut()
                        {
                            affinity.complete_artifacts();
                        }
                        // Earlier completion-tail frames still flow normally. Only the actual
                        // terminal handoff publishes a fully replayable output; a later failed
                        // handoff leaves a harmless hint, never proof that the client received it.
                        if Some(index) == terminal_index
                            && !sender.is_closed()
                            && let Some(affinity) = affinity.as_mut()
                        {
                            if let Some(projector) = renderer.take_projection() {
                                affinity.complete_projection(projector);
                            }
                        }
                        if sender.send(Ok(frame.into())).await.is_err() {
                            // The client's body channel is gone: a client abort, keyed only on
                            // what was actually enqueued before it broke.
                            api_logs::finalize_api_log(
                                api_log_store.as_ref(),
                                &log,
                                ApiLogOutcome::ClientAborted,
                                declared_status(is_status_declared),
                                iteration_usage.or(usage),
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
                            iteration_usage.or(usage),
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
                        iteration_usage.or(usage),
                    )
                    .await;

                    return;
                }
            }
        }

        if sender.is_closed() {
            api_logs::finalize_api_log(
                api_log_store.as_ref(),
                &log,
                ApiLogOutcome::ClientAborted,
                declared_status(is_status_declared),
                iteration_usage.or(usage),
            )
            .await;
            return;
        }
        if has_completed_output && let Some(affinity) = affinity.as_mut() {
            if let Some(projector) = renderer.take_projection() {
                affinity.complete_projection(projector);
            }
        }

        // A clean end of stream: every event rendered and the body closed normally.
        api_logs::finalize_api_log(
            api_log_store.as_ref(),
            &log,
            ApiLogOutcome::Completed,
            Some(i64::from(StatusCode::OK.as_u16())),
            iteration_usage.or(usage),
        )
        .await;
        drop(affinity);
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
/// usage the stream delivered — the terminal usage of a clean end, and for a failed stream the
/// last usage observation it reported before failing (`None` when it delivered none), so a failed
/// non-streaming dispatch records the usage the upstream already delivered exactly as its
/// streaming twin does (README §3 *ApiLog*, §6.2's last-observation rule).
///
/// # Errors
///
/// Returns the stream's failure beside the usage observed before it.
async fn aggregated_response<TBody>(
    events: IrEventStream,
    render_json_fn: impl FnOnce(&IrAggregation) -> TBody,
) -> Result<(Response, Option<IrUsage>), (Error, Option<IrUsage>)>
where
    TBody: Serialize,
{
    let mut aggregator = Aggregator::default();

    // The last usage the stream reported, kept beside either way the stream settles: a mid-stream
    // failure does not undo the usage the upstream already delivered.
    let mut last_usage: Option<IrUsage> = None;
    let mut iteration_usage = None;

    let mut events = events;

    while let Some(item) = events.next().await {
        match item {
            Ok(event) => {
                if let IrEvent::Usage(reported) = &event {
                    last_usage = Some(*reported);
                }

                if let IrEvent::CompactionMetadata { iterations, .. } = &event {
                    iteration_usage =
                        crate::compaction::accounting::reported_iteration_totals(iterations);
                }
                aggregator.ingest(&event);
            }

            Err(error) => return Err((Error::from(error), iteration_usage.or(last_usage))),
        }
    }

    let aggregation = aggregator.finish();
    let usage = crate::compaction::accounting::reported_iteration_totals(
        &aggregation.compaction_iterations,
    )
    .or(aggregation.usage);

    Ok((
        json_response(StatusCode::OK, &render_json_fn(&aggregation)),
        usage,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        format::{UpstreamFormat, openai::chat_completion::Format as ChatCompletionFormat},
        ir::IrFinishReason,
        provider::StoredCredentials,
        storage::{
            api_logs::list_api_logs,
            models::{ModelCreate, insert_model_checked},
        },
        test_support::{
            TempDbGuard, TempDirGuard, create_test_database, insert_provider,
            insert_static_key_definition,
        },
    };
    use std::num::NonZero;
    use tokio::sync::{mpsc, oneshot};

    /// Builds a handler context over the given database with no api-log store:
    /// the compact pipeline's placeholder failure is pre-dispatch, so no store
    /// is ever consulted.
    ///
    /// # Panics
    ///
    /// Panics if an HTTP client fails to build, which the static configuration
    /// prevents.
    fn context(database: crate::database::Database) -> Context {
        Context {
            database,
            api_log_store: None,
            affinity_cache: std::sync::Arc::new(crate::affinity::Cache::default()),
            quota_coordinator: std::sync::Arc::new(
                crate::provider::quota::coordinator::Coordinator::default(),
            ),
            data_plane: crate::http_client::data_plane(),
            control_plane: crate::http_client::control_plane(),
        }
    }

    /// Capacity of the fixture event channel: wide enough that every fixture send completes
    /// without awaiting the consumer.
    const EVENT_CHANNEL_BUFFER: usize = 8;

    /// Creates an api-log store over a fresh temporary directory, with its cleanup guard.
    ///
    /// # Panics
    ///
    /// Panics if the literal retention is zero, which the positive literal prevents.
    fn test_store() -> (api_logs::ApiLogStore, TempDirGuard) {
        let directory =
            std::env::temp_dir().join(format!("janusgate-serving-test-{}", rand::random::<u64>()));

        #[expect(clippy::unwrap_used, reason = "non-zero test retention")]
        let retention = NonZero::new(90).unwrap();

        (
            api_logs::ApiLogStore::new(directory.clone(), retention),
            TempDirGuard(directory),
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
            model_route_id: 7,
            upstream_http_status: None,
            progress: crate::storage::api_logs::AttemptProgress::default(),
            endpoint_type: ApiLogEndpointType::OpenaiChatCompletions,
            is_stream: false,
            billed_usage: None,
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

    /// Sends an event sequence through the fixture channel in order.
    ///
    /// # Panics
    ///
    /// Panics if a fixture send fails, which the wide buffer and the task-owned receiver prevent.
    async fn send_events(
        sender: &mpsc::Sender<Result<IrEvent, gateway::Error>>,
        events: impl IntoIterator<Item = Result<IrEvent, gateway::Error>>,
    ) {
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

    /// The usage observation fixture shared by the clean and failing-with-usage sequences.
    fn usage_event() -> IrEvent {
        IrEvent::Usage(IrUsage {
            input_tokens: 11,
            output_tokens: 7,
            cached_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
        })
    }

    /// The mid-stream upstream failure sequence — one start event, the transport failure, the
    /// pump's terminal `Done`.
    fn failing_events() -> [Result<IrEvent, gateway::Error>; 3] {
        [
            Ok(IrEvent::MessageStart {
                id: "resp-1".to_string(),
                model: "upstream-model".to_string(),
            }),
            Err(gateway::Error::Unavailable {
                detail: "upstream stream failed: broken".to_string(),
            }),
            Ok(IrEvent::Done),
        ]
    }

    /// Sends one clean completion's event sequence — start, text, usage, finish, done — through
    /// the fixture channel and closes the stream.
    ///
    /// # Panics
    ///
    /// Panics if a fixture send fails, which the wide buffer and the task-owned receiver prevent.
    async fn send_clean_events(sender: &mpsc::Sender<Result<IrEvent, gateway::Error>>) {
        send_events(
            sender,
            [
                IrEvent::MessageStart {
                    id: "resp-1".to_string(),
                    model: "upstream-model".to_string(),
                },
                IrEvent::TextDelta("hello".to_string()),
                usage_event(),
                IrEvent::finish(IrFinishReason::Stop),
                IrEvent::Done,
            ]
            .map(Ok),
        )
        .await;
    }

    /// Sends a mid-stream upstream failure — one start event, the transport failure, the pump's
    /// terminal `Done` — through the fixture channel and closes the stream.
    ///
    /// # Panics
    ///
    /// Panics if a fixture send fails, which the wide buffer and the task-owned receiver prevent.
    async fn send_failing_events(sender: &mpsc::Sender<Result<IrEvent, gateway::Error>>) {
        send_events(sender, failing_events()).await;
    }

    /// Sends a mid-stream upstream failure that first delivers a usage observation — one start
    /// event, the usage report, the transport failure, the pump's terminal `Done` — through the
    /// fixture channel and closes the stream.
    ///
    /// # Panics
    ///
    /// Panics if a fixture send fails, which the wide buffer and the task-owned receiver prevent.
    async fn send_failing_events_with_usage(
        sender: &mpsc::Sender<Result<IrEvent, gateway::Error>>,
    ) {
        let [start, tail @ ..] = failing_events();

        send_events(sender, [start, Ok(usage_event())].into_iter().chain(tail)).await;
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
            None,
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
            None,
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
            None,
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

        // The failing stream delivered no usage observation: the five token columns stay `NULL`.
        assert_eq!(row.input_tokens, None);
        assert_eq!(row.output_tokens, None);
        assert_eq!(row.cached_tokens, None);
        assert_eq!(row.cache_write_tokens, None);
        assert_eq!(row.reasoning_tokens, None);

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
            None,
        );

        drop(departed_outcome_receiver);

        send_failing_events(&departed_events_sender).await;
        drop(departed_events_sender);

        departed_task.await;

        let departed_row = recorded_row(&departed_store).await;

        assert_eq!(departed_row.outcome, ApiLogOutcome::UpstreamError);
        assert_eq!(departed_row.http_status, Some(502));

        assert_eq!(departed_row.input_tokens, None);
        assert_eq!(departed_row.output_tokens, None);
        assert_eq!(departed_row.cached_tokens, None);
        assert_eq!(departed_row.cache_write_tokens, None);
        assert_eq!(departed_row.reasoning_tokens, None);
    }

    /// Verifies the detached non-streaming finalization records the usage a failed stream
    /// delivered before its failure (README §3 *ApiLog*, §6.2's last-observation rule): an
    /// upstream that reports usage and then fails mid-generation finalizes the classified
    /// `upstream_error` row carrying that usage — the same attribution its streaming twin
    /// records, and `NULL` columns only when the stream delivered no usage at all.
    ///
    /// # Panics
    ///
    /// Panics if the finalization contract diverges.
    #[tokio::test]
    async fn finalizes_the_usage_a_failed_stream_delivered_before_its_failure() {
        let (store, _guard) = test_store();
        let (events_sender, events) = event_channel();

        let (outcome_sender, _outcome_receiver) = oneshot::channel();

        let task = aggregated_response_task::<ChatCompletionFormat>(
            events,
            "public-model".to_string(),
            Some(store.clone()),
            stamped_draft(),
            log_context(),
            outcome_sender,
            None,
        );

        send_failing_events_with_usage(&events_sender).await;
        drop(events_sender);

        task.await;

        let row = recorded_row(&store).await;

        assert_eq!(row.outcome, ApiLogOutcome::UpstreamError);
        assert_eq!(row.http_status, Some(502));
        assert_eq!(row.input_tokens, Some(11));
        assert_eq!(row.output_tokens, Some(7));
        assert_eq!(row.cached_tokens, None);
        assert_eq!(row.cache_write_tokens, None);
        assert_eq!(row.reasoning_tokens, None);
    }

    /// Builds one static-key provider of the Responses format with an enabled
    /// model whose public id equals `upstream_id`, over a fresh test database.
    ///
    /// # Panics
    ///
    /// Panics if a fixture insert fails, which the fixture prevents.
    async fn routed_model_database() -> (crate::database::Database, TempDbGuard) {
        let (database, guard) = create_test_database().await;

        let definition =
            insert_static_key_definition(&database, "static-key", UpstreamFormat::OpenAiResponse)
                .await;

        let provider = insert_provider(
            &database,
            &definition,
            "mock",
            Some("https://mock.test"),
            StoredCredentials::ApiKey {
                api_key: "upstream-secret".to_string(),
            },
        )
        .await;

        #[expect(clippy::expect_used, reason = "the fixture model inserts")]
        let model = insert_model_checked(
            &database,
            ModelCreate {
                provider_id: provider.id,
                upstream_id: "test-model",
                is_enabled: true,
                thinking_levels_override: None,
                max_input_tokens_override: None,
                max_output_tokens_override: None,
                compaction_support_override: None,
            },
        )
        .await
        .expect("the fixture model inserts");
        #[expect(clippy::expect_used, reason = "the fixture route publishes")]
        crate::storage::routing::create_route(
            &database,
            "test-model",
            true,
            &crate::routing::ModelRouteTarget::Direct {
                model_id: model.model.id,
            },
        )
        .await
        .expect("fixture route publishes");

        (database, guard)
    }

    /// The compact pipeline's pre-dispatch rejections classify through the
    /// shared taxonomy exactly as the ordinary completion pipeline's: a
    /// malformed body is the surface's 400, an unroutable model the 404.
    ///
    /// # Panics
    ///
    /// Panics if a rejection lands outside its contract classification.
    #[tokio::test]
    async fn compact_rejections_classify_through_the_shared_taxonomy() {
        let (database, _guard) = create_test_database().await;

        let context = context(database);

        #[expect(
            clippy::expect_used,
            reason = "the malformed body is constructed to be rejected"
        )]
        let error = run_compact(&context, b"{not json", 1)
            .await
            .expect_err("a malformed body must be rejected");

        assert!(matches!(error, Error::MalformedRequest { .. }));

        assert_eq!(error.http_status(), StatusCode::BAD_REQUEST);

        let body = serde_json::json!({"model": "ghost", "input": "hi"});

        #[expect(
            clippy::expect_used,
            reason = "the unknown model is constructed to be rejected"
        )]
        let error = run_compact(&context, body.to_string().as_bytes(), 1)
            .await
            .expect_err("an unknown model must be rejected");

        assert!(matches!(error, Error::ModelNotFound { .. }));

        assert_eq!(error.http_status(), StatusCode::NOT_FOUND);
    }

    /// A parsed, routed compact request reaches the real executor: the
    /// fixture's single user turn is mandatory-retained wholesale by the
    /// checkpoint planner, leaving no older history to summarize, so the
    /// request fails with the budget-exceeded rejection — a client fault at
    /// 400 whose stable code rides the rendered message — after parse and
    /// routing both succeeded, and with no dispatch, no accounting row exists
    /// for it (README §3 *ApiLog*).
    ///
    /// # Panics
    ///
    /// Panics if the outcome diverges from the documented rejection.
    #[tokio::test]
    async fn a_routed_compact_request_reaches_the_executor() {
        let (database, _guard) = routed_model_database().await;

        let context = context(database);

        // The compact parse itself stays fully functional: the same valid body
        // parses and routes, which the compaction rejection — not a parse or
        // routing failure — proves.
        let body = serde_json::json!({"model": "test-model", "input": "hi"});

        #[expect(clippy::expect_used, reason = "the fixture is constructed to fail")]
        let error = run_compact(&context, body.to_string().as_bytes(), 1)
            .await
            .expect_err("the planner must reject a window with nothing to summarize");

        assert_eq!(error.http_status(), StatusCode::BAD_REQUEST);

        assert!(
            error.to_string().contains("compaction_budget_exceeded"),
            "the stable code rides the rendered message: {error}"
        );
    }

    /// A routed compact request whose model routes through a non-Responses
    /// definition still reaches the executor: routing is format-agnostic, and
    /// the compact parse never inspects the definition.
    ///
    /// # Panics
    ///
    /// Panics if the outcome diverges from the placeholder contract.
    #[tokio::test]
    async fn compact_request_routing_is_format_agnostic() {
        let (database, _guard) = routed_model_database().await;

        let context = context(database);

        // An item-array input with state parses exactly as on the ordinary
        // endpoint before routing: the state vocabulary is shared.
        let body = serde_json::json!({
            "model": "test-model",
            "input": [
                {"type": "message", "role": "user", "content": "hi"},
                {"type": "compaction_trigger"},
            ],
        });

        // The trigger is valid on the ordinary endpoint, but the compact
        // endpoint rejects it before routing — an ambiguity beside the route's
        // own standalone control.
        #[expect(
            clippy::expect_used,
            reason = "the trigger body is constructed to be rejected"
        )]
        let error = run_compact(&context, body.to_string().as_bytes(), 1)
            .await
            .expect_err("a trigger on the compact endpoint must be rejected");

        assert!(matches!(error, Error::MalformedRequest { .. }));

        assert_eq!(error.http_status(), StatusCode::BAD_REQUEST);
    }
    /// Cancellation during internal compaction finalizes one row with the latest billed usage.
    ///
    /// # Panics
    ///
    /// Panics if the asynchronous finalization does not produce the expected row.
    #[tokio::test]
    async fn compaction_cancellation_preserves_billed_work() {
        let (store, _directory) = test_store();
        let mut draft = stamped_draft();
        draft.stamp_billed_usage(Some(IrUsage {
            input_tokens: 20,
            output_tokens: 5,
            ..IrUsage::default()
        }));
        drop(CompactionLogGuard {
            draft,
            store: Some(store.clone()),
            has_transferred: false,
        });
        #[expect(clippy::expect_used, reason = "a dispatched draft finalizes promptly")]
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while recorded_rows(&store).await.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancellation row");
        let row = recorded_row(&store).await;
        assert_eq!(row.outcome, ApiLogOutcome::ClientAborted);
        assert_eq!(row.http_status, None);
        assert_eq!(row.input_tokens, Some(20));
        assert_eq!(row.output_tokens, Some(5));
    }

    /// Native iteration reports replace top-level usage on both aggregated and streaming paths.
    ///
    /// # Panics
    ///
    /// Panics if either response path double-counts or loses the reported compaction work.
    #[tokio::test]
    async fn native_compaction_iterations_are_accounted_once_on_both_response_paths() {
        let iterations =
            vec![serde_json::json!({"type":"compaction","input_tokens":20,"output_tokens":5})];
        for is_streaming in [false, true] {
            let (store, _directory) = test_store();
            let events: IrEventStream = Box::pin(futures::stream::iter(vec![
                Ok(IrEvent::MessageStart {
                    id: "native".into(),
                    model: "public".into(),
                }),
                Ok(IrEvent::Usage(IrUsage::default())),
                Ok(IrEvent::CompactionMetadata {
                    is_paused: true,
                    iterations: iterations.clone(),
                }),
                Ok(IrEvent::finish(IrFinishReason::Stop)),
                Ok(IrEvent::Done),
            ]));
            if is_streaming {
                let response = sse_response(
                    events,
                    Box::new(crate::format::anthropic::messages::SseRenderer::new(
                        "public".into(),
                    )),
                    Some(store.clone()),
                    stamped_draft(),
                    None,
                );
                #[expect(clippy::expect_used, reason = "the healthy fixture stream reads")]
                let _ = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("stream");
            } else {
                let (sender, receiver) = oneshot::channel();
                aggregated_response_task::<crate::format::anthropic::messages::Format>(
                    events,
                    "public".into(),
                    Some(store.clone()),
                    stamped_draft(),
                    log_context(),
                    sender,
                    None,
                )
                .await;
                #[expect(
                    clippy::expect_used,
                    reason = "the healthy fixture response is handed off"
                )]
                let _ = receiver
                    .await
                    .expect("response")
                    .expect("successful response");
            }
            #[expect(
                clippy::expect_used,
                reason = "a completed response finalizes promptly"
            )]
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while recorded_rows(&store).await.is_empty() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("completed row");
            let row = recorded_row(&store).await;
            assert_eq!(row.input_tokens, Some(20));
            assert_eq!(row.output_tokens, Some(5));
        }
    }
    /// Failure before and after response ownership transfer records exactly one compaction row.
    ///
    /// # Panics
    ///
    /// Panics if the coordinator loses billed work or duplicates a failed request's row.
    #[tokio::test]
    async fn compaction_failures_finalize_once_before_and_after_response_transfer() {
        for should_continue in [false, true] {
            let (database, _database_guard) = routed_model_database().await;
            let (store, _directory) = test_store();
            let content = if should_continue { "summary" } else { "" };
            let body = serde_json::json!({"id":"native",
"model":"test-model",
"status":"completed",
"output":[{"type":"message",
"role":"assistant",
"content":[{"type":"output_text",
"text":content}]}],
"usage":{"input_tokens":20,
"output_tokens":5}});
            let mut actions = vec![crate::test_support::MockAction::respond_json(
                body.to_string(),
            )];
            if should_continue {
                actions.push(crate::test_support::MockAction::respond_json(
                    "malformed JSON".into(),
                ));
            }
            let (mock, _requests) =
                crate::test_support::MockUpstream::start_scripted_recording_bodies(actions).await;
            let mut context = context(database);
            context.api_log_store = Some(store.clone());
            #[expect(
                clippy::expect_used,
                reason = "the fixture routing and compaction settings exist"
            )]
            let mut routing = gateway::resolve_routing(&context.database, "test-model")
                .await
                .expect("routing");
            routing.provider.base_url = Some(mock.base());
            #[expect(
                clippy::expect_used,
                reason = "the fixture declares valid compaction settings"
            )]
            {
                routing.provider.definition.payload.compaction = Some(
                    serde_json::from_value(
                        serde_json::json!({"strategy":"gateway_summary","retention_budget":1}),
                    )
                    .expect("section"),
                );
            }
            let request = serde_json::json!({
                "model": "test-model",
                "input": [
                    {"type": "message", "role": "user", "content": "old history"},
                    {"type": "message", "role": "user", "content": "pending"}
                ],
                "context_management": [{"type": "compaction", "compact_threshold": 1}]
            });
            #[expect(clippy::expect_used, reason = "the automatic request is valid")]
            let parsed = crate::format::openai::response::Format::parse_request(
                request.to_string().as_bytes(),
                &RouteContext::empty(),
            )
            .expect("request");
            #[expect(clippy::expect_used, reason = "the fixture's state authenticates")]
            let preparation = prepare_compaction_boundaries(&parsed, "completion")
                .expect("authenticated compaction state");
            let outcome = run_compaction_pipeline::<crate::format::openai::response::Format>(
                &context,
                parsed,
                preparation,
                gateway::CompactionSelection::direct(routing),
                stamped_draft(),
                log_context(),
                false,
                true,
                "test-model".into(),
                OperationKind::Completion,
            )
            .await;
            assert!(outcome.is_err());
            let row = recorded_row(&store).await;
            assert_eq!(row.outcome, ApiLogOutcome::UpstreamError);
            assert_eq!(row.input_tokens, Some(20));
            assert_eq!(row.output_tokens, Some(5));
        }
    }

    /// Completed JSON output becomes a history anchor before handoff; known departures and
    /// failed bodies retain only their request retry hint, never a completed output anchor.
    /// Namespace deletion fences the detached task's later callback as well.
    ///
    /// # Errors
    ///
    /// Returns fixture parser, channel, or task failures.
    ///
    /// # Panics
    ///
    /// Panics if result ownership or completion/disconnect anchoring diverges.
    #[tokio::test]
    #[expect(
        clippy::await_holding_lock,
        reason = "affinity fixtures serialize configuration"
    )]
    async fn json_history_registration_obeys_completion_and_owned_task_lifetime()
    -> Result<(), Box<dyn std::error::Error>> {
        let _configuration = crate::test_support::configuration_lock();
        crate::test_support::install_global_configuration();
        for (is_departed, has_failure, is_deleted) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let cache = std::sync::Arc::new(crate::affinity::Cache::default());
            let namespace = crate::affinity::Namespace {
                api_key_id: 3,
                pool_id: 11,
            };
            let original = ChatCompletionFormat::parse_request(
                br#"{
                "model":"M","messages":[{"role":"user","content":"input"}]}
                "#,
                &RouteContext::empty(),
            )?;
            let projector = original.affinity.project(&original.ir.messages);
            let evidence = projector.evidence();
            #[expect(clippy::panic, reason = "a wrong decision is the test failure report")]
            let crate::affinity::Decision::Fresh(lease) = cache.acquire(namespace, &evidence)
            else {
                panic!("new request establishment");
            };
            let affinity = crate::affinity::Accepted::new(lease.commit(5), projector);
            let (sender, events) = event_channel();
            let (outcome_sender, outcome_receiver) = oneshot::channel();
            let task = tokio::spawn(aggregated_response_task::<ChatCompletionFormat>(
                events,
                "M".into(),
                None,
                stamped_draft(),
                log_context(),
                outcome_sender,
                Some(affinity),
            ));
            let outcome_receiver = if is_departed {
                drop(outcome_receiver);
                None
            } else {
                Some(outcome_receiver)
            };
            if is_deleted {
                cache.remove_api_key(namespace.api_key_id);
            }
            if has_failure {
                send_events(
                    &sender,
                    [
                        Ok(IrEvent::TextDelta("hello".into())),
                        Err(gateway::Error::Unavailable {
                            detail: "failed".into(),
                        }),
                        Ok(IrEvent::Done),
                    ],
                )
                .await;
            } else {
                send_clean_events(&sender).await;
            }
            drop(sender);
            if let Some(receiver) = outcome_receiver {
                let response = receiver.await?;
                assert_eq!(response.is_err(), has_failure);
                if let Ok(response) = response {
                    let body = axum::body::to_bytes(response.into_body(), 262_144).await?;
                    let body: serde_json::Value = serde_json::from_slice(&body)?;
                    assert_eq!(
                        body.pointer("/choices/0/message/content"),
                        Some(&serde_json::Value::String("hello".into()))
                    );
                }
            }
            task.await?;
            let replay = ChatCompletionFormat::parse_request(
                br#"{
                "model":"changed-public-name","messages":[
                    {"role":"user","content":"input"},
                    {"role":"assistant","content":"hello"},
                    {"role":"user","content":"next"}]}
                "#,
                &RouteContext::empty(),
            )?;
            let replay = replay.affinity.project(&replay.ir.messages).evidence();
            match cache.acquire(namespace, &replay) {
                crate::affinity::Decision::Bound(binding) => {
                    assert!(!is_departed && !has_failure && !is_deleted);
                    assert_eq!(binding.provider_id(), 5);
                }
                crate::affinity::Decision::Fresh(_) => {
                    assert!(is_departed || has_failure || is_deleted);
                }
                #[expect(clippy::panic, reason = "a wrong decision is the test failure report")]
                _ => panic!("replay is an established completed output or a fresh miss"),
            }
            if !is_deleted {
                #[expect(clippy::panic, reason = "a wrong decision is the test failure report")]
                let crate::affinity::Decision::Bound(binding) = cache.acquire(namespace, &evidence)
                else {
                    panic!("accepted headers retain their independent retry hint");
                };
                assert_eq!(binding.provider_id(), 5);
            }
        }
        Ok(())
    }

    /// Prepared prefixes and live native checkpoints register at their block stop and survive a
    /// later failure, and suppress hints after an already known departure or namespace
    /// deletion.
    ///
    /// # Errors
    ///
    /// Returns fixture parser, channel, or response-body failures.
    ///
    /// # Panics
    ///
    /// Panics if the fixture lacks its artifact or violates delivery ownership.
    #[tokio::test]
    #[expect(
        clippy::await_holding_lock,
        reason = "affinity fixtures serialize configuration"
    )]
    async fn prepared_checkpoint_registration_obeys_block_delivery()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::format::anthropic::messages::SseRenderer as MessagesRenderer;
        let _configuration = crate::test_support::configuration_lock();
        crate::test_support::install_global_configuration();
        for client_format in [
            "messages",
            "messages_checkpoint",
            "responses_checkpoint",
            "messages_native",
        ] {
            for is_threshold in [false, true] {
                for (has_failure, is_departed, is_deleted) in [
                    (false, false, false),
                    (true, false, false),
                    (false, true, false),
                    (false, false, true),
                ] {
                    let cache = std::sync::Arc::new(crate::affinity::Cache::default());
                    let namespace = crate::affinity::Namespace {
                        api_key_id: 3,
                        pool_id: 11,
                    };
                    let original = ChatCompletionFormat::parse_request(
                        br#"{"model":"M","messages":[{"role":"user","content":"input"}]}"#,
                        &RouteContext::empty(),
                    )?;
                    let projector = original.affinity.project(&original.ir.messages);
                    #[expect(
                        clippy::panic,
                        reason = "unexpected fixture establishment fails the test"
                    )]
                    let crate::affinity::Decision::Fresh(lease) =
                        cache.acquire(namespace, &projector.evidence())
                    else {
                        panic!("fresh prefix fixture");
                    };
                    let block = if client_format == "responses_checkpoint" {
                        serde_json::json!({"type":"compaction","encrypted_content":"native state"})
                    } else {
                        serde_json::json!({"type":"compaction","content":"summary",
                        "signature":"native signature","tool_changes":[{"z":2,"a":1}]})
                    };
                    #[expect(
                        clippy::expect_used,
                        reason = "the fixture uses accepted checkpoint fields"
                    )]
                    let (window, anchors) = if client_format == "responses_checkpoint" {
                        let window =
                            crate::format::openai::response::gateway_checkpoint_projection(
                                std::slice::from_ref(&block),
                            )
                            .expect("accepted Responses state");
                        let mut anchors = window.completed_anchors();
                        anchors.push(crate::affinity::Anchor::Artifact(
                            window.native_artifact(&block),
                        ));
                        (window, anchors)
                    } else {
                        crate::format::anthropic::messages::completed_compaction_projection(
                            &block, None,
                        )
                        .expect("accepted Messages state")
                    };
                    #[expect(
                        clippy::expect_used,
                        reason = "the valid fixture block supplies its artifact"
                    )]
                    let artifact = anchors
                        .iter()
                        .find_map(|anchor| match anchor {
                            crate::affinity::Anchor::Artifact(digest) => Some(*digest),
                            crate::affinity::Anchor::History(_) => None,
                        })
                        .expect("native artifact");
                    let affinity = crate::affinity::Accepted::new(lease.commit(5), projector);
                    let native_payload = block.clone();
                    let affinity = if client_format == "messages_native" {
                        affinity
                    } else if client_format == "messages" {
                        affinity.completed_prefix(anchors)
                    } else {
                        affinity.completed_checkpoint(window, anchors)
                    };
                    let (sender, events) = event_channel();
                    let renderer: Box<dyn ClientSseRenderer + Send> =
                        if client_format == "responses_checkpoint" {
                            Box::new(
                                crate::format::openai::response::SseRenderer::new("M".into())
                                    .with_prefix_items(vec![block]),
                            )
                        } else if client_format == "messages_native" {
                            Box::new(
                                MessagesRenderer::new("M".into())
                                    .with_threshold_compaction(is_threshold),
                            )
                        } else {
                            Box::new(
                                MessagesRenderer::with_compaction_prefix(
                                    "M".into(),
                                    vec![block],
                                    Some("compaction".into()),
                                )
                                .with_threshold_compaction(is_threshold),
                            )
                        };
                    let response =
                        sse_response(events, renderer, None, stamped_draft(), Some(affinity));
                    let mut body = response.into_body().into_data_stream();
                    if is_deleted {
                        cache.remove_api_key(namespace.api_key_id);
                    }
                    if is_departed {
                        drop(body);
                        send_events(
                            &sender,
                            [
                                Ok(IrEvent::MessageStart {
                                    id: "upstream".into(),
                                    model: "physical".into(),
                                }),
                                Ok(if client_format == "messages_native" {
                                    IrEvent::CompactionState {
                                        payload: native_payload.clone(),
                                    }
                                } else {
                                    IrEvent::Done
                                }),
                                Ok(IrEvent::Done),
                            ],
                        )
                        .await;
                        drop(sender);
                        tokio::task::yield_now().await;
                    } else {
                        send_events(
                            &sender,
                            [
                                Ok(IrEvent::MessageStart {
                                    id: "upstream".into(),
                                    model: "physical".into(),
                                }),
                                Ok(IrEvent::Usage(IrUsage::default())),
                                Ok(if client_format == "messages_native" {
                                    IrEvent::CompactionState {
                                        payload: native_payload.clone(),
                                    }
                                } else {
                                    IrEvent::Usage(IrUsage::default())
                                }),
                            ],
                        )
                        .await;
                        loop {
                            #[expect(
                                clippy::expect_used,
                                reason = "the fixture emits a complete prefix"
                            )]
                            let frame = body.next().await.expect("prefix frame")?;
                            let boundary = if client_format == "responses_checkpoint" {
                                "event: response.output_item.done"
                            } else {
                                "event: content_block_stop"
                            };
                            if std::str::from_utf8(&frame)?.contains(boundary) {
                                break;
                            }
                        }
                        if has_failure {
                            send_events(
                                &sender,
                                [Err(gateway::Error::Unavailable {
                                    detail: "later failure".into(),
                                })],
                            )
                            .await;
                        } else {
                            send_events(&sender, [Ok(IrEvent::Done)]).await;
                        }
                        drop(sender);
                        while let Some(frame) = body.next().await {
                            frame?;
                        }
                    }
                    let evidence = crate::affinity::Evidence {
                        artifact: Some(artifact),
                        ..crate::affinity::Evidence::default()
                    };
                    match cache.acquire(namespace, &evidence) {
                        crate::affinity::Decision::Bound(binding) => {
                            assert!(!is_departed && !is_deleted);
                            assert_eq!(binding.provider_id(), 5);
                        }
                        crate::affinity::Decision::Fresh(_) => assert!(is_departed || is_deleted),
                        #[expect(
                            clippy::panic,
                            reason = "an unexpected fixture decision fails the test"
                        )]
                        _ => panic!("unexpected prefix affinity decision"),
                    }
                }
            }
        }
        Ok(())
    }

    /// Streaming completion registers actual outward history at its terminal frame, while an
    /// upstream error, known disconnect, or namespace deletion cannot create that output hint.
    ///
    /// # Errors
    ///
    /// Returns fixture parser, channel, or response-body failures.
    ///
    /// # Panics
    ///
    /// Panics if terminal commitment or known cancellation changes affinity ownership.
    #[tokio::test]
    #[expect(
        clippy::await_holding_lock,
        reason = "affinity fixtures serialize configuration"
    )]
    async fn streamed_history_registration_obeys_terminal_delivery()
    -> Result<(), Box<dyn std::error::Error>> {
        let _configuration = crate::test_support::configuration_lock();
        crate::test_support::install_global_configuration();
        for client_format in ["chat", "generate", "responses", "messages", "interactions"] {
            for (has_failure, is_departed, is_deleted) in [
                (false, false, false),
                (true, false, false),
                (false, true, false),
                (false, false, true),
            ] {
                let (store, _directory) = test_store();
                let cache = std::sync::Arc::new(crate::affinity::Cache::default());
                let namespace = crate::affinity::Namespace {
                    api_key_id: 3,
                    pool_id: 11,
                };
                let original = ChatCompletionFormat::parse_request(
                    br#"{
                "model":"M","messages":[{"role":"user","content":"input"}]}"#,
                    &RouteContext::empty(),
                )?;
                let projector = original.affinity.project(&original.ir.messages);
                let evidence = projector.evidence();
                #[expect(
                    clippy::panic,
                    reason = "an unexpected decision is the test failure report"
                )]
                let crate::affinity::Decision::Fresh(lease) = cache.acquire(namespace, &evidence)
                else {
                    panic!("new request establishment");
                };
                let native_item = serde_json::json!({"type":"compaction","id":"cmp_complete",
                    "encrypted_content":"native state"});
                let artifact = projector.native_artifact(&native_item);
                let affinity = crate::affinity::Accepted::new(lease.commit(5), projector);
                let (sender, events) = event_channel();
                let response = sse_response(
                    events,
                    match client_format {
                        "generate" => {
                            crate::format::gemini::generate_content::Format::sse_renderer(
                                false,
                                "M".into(),
                            )
                        }
                        "responses" => {
                            crate::format::openai::response::Format::sse_renderer(false, "M".into())
                        }
                        "messages" => crate::format::anthropic::messages::Format::sse_renderer(
                            false,
                            "M".into(),
                        ),
                        "interactions" => crate::format::gemini::interaction::Format::sse_renderer(
                            false,
                            "M".into(),
                        ),
                        _ => ChatCompletionFormat::sse_renderer(true, "M".into()),
                    },
                    Some(store.clone()),
                    stamped_draft(),
                    Some(affinity),
                );
                send_events(
                    &sender,
                    [
                        Ok(IrEvent::MessageStart {
                            id: "upstream".into(),
                            model: "physical-model".into(),
                        }),
                        Ok(if client_format == "responses" {
                            IrEvent::Usage(IrUsage::default())
                        } else {
                            IrEvent::TextDelta("hello".into())
                        }),
                    ],
                )
                .await;
                let mut body = response.into_body().into_data_stream();
                for _ in 0..match client_format {
                    "generate" => 1,
                    "responses" => 2,
                    "messages" | "interactions" => 3,
                    _ => 2,
                } {
                    #[expect(
                        clippy::expect_used,
                        reason = "the fixture emits its role and text frames"
                    )]
                    body.next().await.expect("intermediate streamed frame")?;
                }
                if is_deleted {
                    cache.remove_api_key(namespace.api_key_id);
                }
                if is_departed {
                    drop(body);
                    if client_format == "responses" {
                        send_events(
                            &sender,
                            [Ok(IrEvent::CompactionState {
                                payload: native_item.clone(),
                            })],
                        )
                        .await;
                    }
                    send_events(&sender, [Ok(IrEvent::Done)]).await;
                    drop(sender);
                    // Let the owned renderer observe its closed client channel and release
                    // metadata.
                    tokio::task::yield_now().await;
                } else {
                    if client_format == "responses" {
                        send_events(
                            &sender,
                            [
                                Ok(IrEvent::CompactionState {
                                    payload: native_item.clone(),
                                }),
                                Ok(IrEvent::TextDelta("hello".into())),
                            ],
                        )
                        .await;
                    }
                    if has_failure {
                        send_events(
                            &sender,
                            [Err(gateway::Error::Unavailable {
                                detail: "failed".into(),
                            })],
                        )
                        .await;
                    } else {
                        send_events(&sender, [Ok(IrEvent::finish(IrFinishReason::Stop))]).await;
                    }
                    send_events(&sender, [Ok(IrEvent::Done)]).await;
                    drop(sender);
                    while let Some(frame) = body.next().await {
                        frame?;
                    }
                }
                let row = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    loop {
                        if let Some(row) = recorded_rows(&store).await.into_iter().next() {
                            break row;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await?;
                assert_eq!(
                    row.outcome,
                    if is_departed {
                        ApiLogOutcome::ClientAborted
                    } else if has_failure {
                        ApiLogOutcome::UpstreamError
                    } else {
                        ApiLogOutcome::Completed
                    }
                );
                assert_eq!(row.http_status, Some(200));
                let replay = ChatCompletionFormat::parse_request(
                    br#"{
                "model":"renamed","messages":[{"role":"user","content":"input"},
                    {"role":"assistant","content":"hello"},{"role":"user","content":"next"}]}"#,
                    &RouteContext::empty(),
                )?;
                let replay = if client_format == "responses" {
                    let replay = crate::format::openai::response::Format::parse_request(
                        serde_json::json!({"model":"renamed","input":[
                            native_item,
                            {"type":"message","role":"assistant","content":"hello"},
                            {"type":"message","role":"user","content":"next"}
                        ]})
                        .to_string()
                        .as_bytes(),
                        &RouteContext::empty(),
                    )?;
                    #[expect(
                        clippy::expect_used,
                        reason = "the fixture contains its native boundary"
                    )]
                    let compaction = replay.compaction.as_ref().expect("native replay boundary");
                    let active =
                        crate::compaction::history::BoundaryPreparation::authenticate(compaction)?
                            .routing_preparation(&replay)?;
                    // Probe only the completed state-plus-answer unit count. The independently
                    // released checkpoint remains usable after failure, but cannot establish an
                    // unfinished answer's full window.
                    crate::affinity::Evidence {
                        history: active
                            .projector
                            .evidence()
                            .history
                            .into_iter()
                            .filter(|prefix| prefix.units.get() == 2)
                            .collect(),
                        ..crate::affinity::Evidence::default()
                    }
                } else {
                    replay.affinity.project(&replay.ir.messages).evidence()
                };
                match cache.acquire(namespace, &replay) {
                    crate::affinity::Decision::Bound(binding) => {
                        assert!(!has_failure && !is_departed && !is_deleted);
                        assert_eq!(binding.provider_id(), 5);
                    }
                    crate::affinity::Decision::Fresh(_) => {
                        assert!(has_failure || is_departed || is_deleted);
                    }
                    #[expect(
                        clippy::panic,
                        reason = "an unexpected decision is the test failure report"
                    )]
                    _ => panic!("completed output binding or a fresh history miss"),
                }
                if client_format == "responses" {
                    match cache.acquire(
                        namespace,
                        &crate::affinity::Evidence {
                            artifact: Some(artifact),
                            ..crate::affinity::Evidence::default()
                        },
                    ) {
                        crate::affinity::Decision::Bound(binding) => {
                            assert!(!is_departed && !is_deleted);
                            assert_eq!(binding.provider_id(), 5);
                        }
                        crate::affinity::Decision::Fresh(_) => assert!(is_departed || is_deleted),
                        #[expect(
                            clippy::panic,
                            reason = "an unexpected decision is the test failure report"
                        )]
                        _ => panic!("complete released artifact survives a later stream failure"),
                    }
                }
                if !is_deleted {
                    #[expect(
                        clippy::panic,
                        reason = "an unexpected decision is the test failure report"
                    )]
                    let crate::affinity::Decision::Bound(binding) =
                        cache.acquire(namespace, &evidence)
                    else {
                        panic!("accepted headers retain their independent retry hint");
                    };
                    assert_eq!(binding.provider_id(), 5);
                }
            }
        }
        Ok(())
    }
}
