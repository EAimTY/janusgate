//! Completion parsing, route selection, compaction coordination, and concrete artifact rendering.

use super::delivery::{
    AggregationRender, detached_aggregation, finalize_buffered, prerendered_sse_response,
    sse_response,
};
use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome},
    api::{Error, log_error},
    compaction::{
        accounting::CompactionLedger,
        executor::{self, MessagesOuterOutcome, ResponsesOuterOutcome},
        history::BoundaryPreparation,
    },
    context::Context,
    format::{
        ClientFormat, ClientSseRenderer, CompactionBoundary, CompactionControl, NativeArtifact,
        ParsedRequest, RouteContext, aggregation_events,
        anthropic::messages::{CompactionOnlyUsage, render_compaction_only_response},
        openai::response::{parse_compact_request, render_trigger_stream_frames},
    },
    gateway::{self, CompactionDispatch, IrEventStream},
    ir::{IrAggregation, IrEvent, IrUsage},
    storage::api_logs::{self, ApiLogDraft},
};
use axum::response::Response;
use serde::Serialize;

/// The closed request-kind vocabulary [`run_completion`] names in its log lines: a completion —
/// every surface's endpoint but one — or a Gemini Interactions interaction (`POST
/// /api/gemini-v1beta/interactions`). [`OperationKind::as_str`] renders the exact log spelling,
/// so the composed log templates stay one vocabulary with the call sites.
#[derive(Debug, Clone, Copy)]
pub(in crate::api) enum OperationKind {
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
pub(in crate::api) async fn run_completion<TFormat>(
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
            detached_aggregation::<TFormat>(
                events,
                public_id,
                context.api_log_store.clone(),
                log,
                log_context.clone(),
                AggregationRender::Ordinary,
                affinity,
            )
            .await
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
pub(super) fn prepare_compaction_boundaries(
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
pub(super) struct CompactionAccounting {
    /// The active draft; delivery takes this value exactly once.
    pub(super) draft: Option<ApiLogDraft>,
    /// Store independent of the departing request.
    pub(super) store: Option<api_logs::ApiLogStore>,
}

impl CompactionAccounting {
    /// Borrows the draft while execution still owns accounting.
    ///
    /// # Panics
    ///
    /// Panics if execution accesses a draft already transferred to delivery.
    fn draft_mut(&mut self) -> &mut ApiLogDraft {
        #[expect(
            clippy::expect_used,
            reason = "execution retains accounting until delivery starts"
        )]
        self.draft.as_mut().expect("execution owns the draft")
    }

    /// Transfers the draft to its final response or failure owner.
    ///
    /// # Panics
    ///
    /// Panics if execution attempts a second ownership transfer.
    pub(super) fn take(&mut self) -> ApiLogDraft {
        #[expect(
            clippy::expect_used,
            reason = "delivery takes the execution draft exactly once"
        )]
        self.draft.take().expect("execution owns the draft")
    }
}

impl Drop for CompactionAccounting {
    fn drop(&mut self) {
        if let Some(draft) = self.draft.take() {
            api_logs::spawn_api_log_finalization(
                self.store.clone(),
                draft,
                ApiLogOutcome::ClientAborted,
                None,
                None,
            );
        }
    }
}

/// Finalizes a compaction error independently of a canceled handler.
async fn finalize_compaction_failure(context: &Context, log: ApiLogDraft, error: &Error) {
    let (outcome, status) = error.accounting();
    if let Some(task) = api_logs::spawn_api_log_finalization(
        context.api_log_store.clone(),
        log,
        outcome,
        Some(status),
        None,
    ) {
        let _ = task.await;
    }
}

/// Coordinates compaction accounting until the response takes ownership of it.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_compaction_pipeline<TFormat>(
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
    let mut guard = CompactionAccounting {
        draft: Some(log),
        store: context.api_log_store.clone(),
    };
    let outcome = loop {
        let routing = selection.routing.clone();
        let outcome = run_compaction_pipeline_inner::<TFormat>(
            context,
            parsed.clone(),
            &preparation,
            &mut selection,
            routing,
            &mut guard,
            log_context.clone(),
            is_streaming,
            should_include_usage,
            public_id.clone(),
            operation,
        )
        .await;
        if matches!(outcome, Err(Error::QuotaRejected { .. })) {
            match selection
                .advance_after_quota_rejection(context, guard.draft_mut())
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
    if let Err(error) = &outcome
        && let Some(draft) = guard.draft.take()
    {
        finalize_compaction_failure(context, draft, error).await;
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
    accounting: &mut CompactionAccounting,
    log_context: String,
    is_streaming: bool,
    should_include_usage: bool,
    public_id: String,
    operation: OperationKind,
) -> Result<Response, Error>
where
    TFormat: ClientFormat + 'static,
    TFormat::JsonResponse: Serialize,
{
    let surface = compaction_surface(&parsed);

    // The ledger's billed totals stamp the accounting row for every outcome
    // that carries one; the executor has already recorded each dispatch into
    // it (README §6.8).
    let stamp_billed = |draft: &mut ApiLogDraft, ledger: Option<&CompactionLedger>| {
        if let Some(ledger) = ledger {
            draft.stamp_billed_usage(ledger.reported_billed_totals());
        }
    };

    if surface == CompactionSurface::Messages {
        let outcome = executor::execute_messages_request(
            context,
            &routing,
            &parsed,
            preparation,
            accounting.draft_mut(),
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
                    accounting,
                    log_context,
                    is_streaming,
                    should_include_usage,
                    public_id.clone(),
                    dispatch,
                    parsed.clone(),
                    preparation,
                    selection,
                    operation,
                )
                .await
            }

            MessagesOuterOutcome::CompactionOnly {
                block,
                artifact_id,
                iterations,
                ledger,
            } => {
                stamp_billed(accounting.draft_mut(), Some(&ledger));

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
                    accounting.take(),
                    affinity,
                )
            }

            MessagesOuterOutcome::NativeNoSummary {
                aggregation,
                ledger,
            } => {
                stamp_billed(accounting.draft_mut(), Some(&ledger));

                aggregated_artifact_response(
                    context,
                    &public_id,
                    &aggregation,
                    is_streaming,
                    should_include_usage,
                    accounting.take(),
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
                stamp_billed(accounting.draft_mut(), Some(&ledger));

                checkpoint_completion::<TFormat>(
                    context,
                    routing,
                    ir,
                    accounting,
                    log_context,
                    is_streaming,
                    should_include_usage,
                    public_id,
                    Vec::new(),
                    prefix_blocks,
                    iterations,
                    overlay_headers,
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
            accounting.draft_mut(),
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
                    accounting,
                    log_context,
                    is_streaming,
                    should_include_usage,
                    public_id.clone(),
                    dispatch,
                    parsed.clone(),
                    preparation,
                    selection,
                    operation,
                )
                .await
            }

            ResponsesOuterOutcome::CompactionOnly {
                items,
                artifact_id,
                usage,
                ledger,
            } => {
                stamp_billed(accounting.draft_mut(), Some(&ledger));

                let response = compaction_only_responses_response(
                    context,
                    &public_id,
                    &items,
                    usage,
                    is_streaming,
                    accounting.take(),
                )?;
                selection.register_gateway_window(&artifact_id, &items);
                Ok(response)
            }

            ResponsesOuterOutcome::NativeLifecycle {
                aggregation,
                ledger,
            } => {
                stamp_billed(accounting.draft_mut(), Some(&ledger));
                let affinity = selection.accepted();
                aggregated_artifact_response(
                    context,
                    &public_id,
                    &aggregation,
                    is_streaming,
                    should_include_usage,
                    accounting.take(),
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
                stamp_billed(accounting.draft_mut(), Some(&ledger));

                checkpoint_completion::<TFormat>(
                    context,
                    routing,
                    ir,
                    accounting,
                    log_context,
                    is_streaming,
                    should_include_usage,
                    public_id,
                    prefix_items,
                    Vec::new(),
                    Vec::new(),
                    overlay_headers,
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
    accounting: &mut CompactionAccounting,
    log_context: String,
    is_streaming: bool,
    should_include_usage: bool,
    public_id: String,
    dispatch: CompactionDispatch,
    fallback_request: ParsedRequest,
    preparation: &BoundaryPreparation,
    selection: &mut gateway::CompactionSelection,
    operation: OperationKind,
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
        accounting.draft_mut(),
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
                    accounting,
                    log_context,
                    is_streaming,
                    should_include_usage,
                    public_id,
                    operation,
                ))
                .await;
            }
            return Err(Error::Upstream { status, message });
        }
        Err(error) => return Err(error.into()),
    };

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
            accounting.take(),
            selection.accepted(),
        ))
    } else {
        detached_aggregation::<TFormat>(
            events,
            public_id,
            context.api_log_store.clone(),
            accounting.take(),
            log_context,
            AggregationRender::Ordinary,
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
    accounting: &mut CompactionAccounting,
    log_context: String,
    is_streaming: bool,
    should_include_usage: bool,
    public_id: String,
    prefix_items: Vec<crate::format::compaction_wire::Checkpoint>,
    prefix_blocks: Vec<crate::format::compaction_wire::Checkpoint>,
    iterations: Vec<crate::format::compaction_wire::UsageIteration>,
    overlay_headers: Vec<crate::provider::definition::template::ParamRow>,
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
        accounting.draft_mut(),
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
            accounting.take(),
            affinity,
        ))
    } else {
        detached_aggregation::<TFormat>(
            events,
            public_id,
            context.api_log_store.clone(),
            accounting.take(),
            log_context,
            AggregationRender::WithPrefix {
                checkpoints: if prefix_items.is_empty() {
                    prefix_blocks
                } else {
                    prefix_items
                },
                iterations,
            },
            affinity,
        )
        .await
    }
}

/// Boxes the Responses streaming renderer carrying checkpoint prefix items.
fn openai_sse_renderer_with_items(
    should_include_usage: bool,
    public_id: String,
    prefix_items: Vec<crate::format::compaction_wire::Checkpoint>,
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
    prefix_blocks: Vec<crate::format::compaction_wire::Checkpoint>,
    iterations: Vec<crate::format::compaction_wire::UsageIteration>,
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

/// Renders the Responses compact-only outcome: the synthesized trigger
/// lifecycle — streaming callers receive the complete frame sequence, JSON
/// callers the same artifact as the aggregated body.
fn compaction_only_responses_response(
    context: &Context,
    public_id: &str,
    items: &[crate::format::compaction_wire::Checkpoint],
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

        Ok(finalize_buffered(context.api_log_store.clone(), log, &body))
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
    block: &crate::format::compaction_wire::Checkpoint,
    iterations: &[crate::format::compaction_wire::UsageIteration],
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
        Ok(finalize_buffered(context.api_log_store.clone(), log, &body))
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
            Ok(finalize_buffered(context.api_log_store.clone(), log, &body))
        } else {
            let body = crate::format::openai::response::Format::render_json_response(
                public_id,
                aggregation,
            );
            if let Some(affinity) = &mut affinity {
                affinity.complete_json::<crate::format::openai::response::Format>(&body);
            }
            Ok(finalize_buffered(context.api_log_store.clone(), log, &body))
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
pub(in crate::api) async fn run_compact(
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

    let mut guard = CompactionAccounting {
        draft: Some(log),
        store: context.api_log_store.clone(),
    };
    let outcome = loop {
        let outcome = executor::execute_responses_compact(
            context,
            &selection.routing,
            &parsed,
            &preparation,
            guard.draft_mut(),
            Some(&mut selection.ownership),
        )
        .await;
        if matches!(outcome, Err(Error::QuotaRejected { .. })) {
            match selection
                .advance_after_quota_rejection(context, guard.draft_mut())
                .await
            {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => break Err(error.into()),
            }
        }
        break outcome;
    };
    let log = guard.take();
    match outcome {
        Ok(body) => {
            selection.register_compacted(&body);
            Ok(finalize_buffered(context.api_log_store.clone(), log, &body))
        }

        Err(error) => {
            finalize_compaction_failure(context, log, &error).await;
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
