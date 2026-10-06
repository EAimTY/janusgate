//! Completion parsing, route selection, compaction coordination, and concrete artifact rendering.

use super::delivery::{
    AggregationRender, CheckpointPrefix, complete_accepted_json, detached_aggregation,
    finalize_buffered, prerendered_sse_response, sse_response,
};
use crate::{
    accounting::ApiLogEndpointType,
    api::{Error, log_error},
    compaction::executor::{self, MessagesOuterOutcome, ResponsesOuterOutcome},
    compaction_model::{accounting::CompactionLedger, boundary::BoundaryPreparation},
    context::Context,
    format::{
        ClientFormat, ClientSseRenderer, CompactionBoundary, CompactionControl, NativeArtifact,
        ParsedRequest, RouteContext, aggregation_events,
        anthropic::messages::{CompactionOnlyUsage, render_compaction_only_response},
        openai::response::{parse_compact_request, render_trigger_stream_frames},
    },
    gateway::{self, CompactionDispatch, IrEventStream},
    ir::{IrAggregation, IrEvent, IrUsage},
    storage::api_logs::{ApiLogDraft, ApiLogGuard},
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

    let public_id = routing.public_id.clone();

    // The request's stable render facts: decided here at the surface boundary
    // and consumed unchanged by every outcome rendering below.
    let render = RenderContext {
        is_streaming: parsed.is_streaming,
        should_include_usage: parsed.should_include_usage,
        public_id,
        log_context: format!(
            "{} for model `{}` via provider `{}`",
            operation.as_str(),
            routing.public_id,
            routing.provider.name
        ),
    };

    // The accounting draft: every fact known before dispatch, built from the authenticated key,
    // the routing row, and the surface's endpoint type; `gateway::execute` stamps the dispatch
    // instant into it (README §3 *ApiLog*).
    let log = ApiLogDraft::predispatch(
        api_key_id,
        routing.provider.id,
        routing.model.id,
        routing.model_route_id,
        endpoint_type,
        render.is_streaming,
    );

    // A request carrying compaction controls or compaction state routes
    // through the compaction pipeline: restore, strategy, and the outcome
    // renderings README §6.8 pins. Every other request runs the ordinary
    // path unchanged. The pipeline logs its own final failure through
    // `render.log_context`, keeping this boundary's one-log-per-failure
    // contract with the resolved provider named.
    run_compaction_pipeline::<TFormat>(
        context,
        parsed,
        preparation,
        selection,
        log,
        render,
        operation,
    )
    .await
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
            gateway::AttemptOutcome::Accepted {
                events,
                mut attempt,
            } => (events, attempt.take()),
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

/// Whether the request's compaction control is the threshold-shaped Messages
/// control: the one control whose ordinary outcome renders through the
/// threshold-aware Messages renderer. Both consults sit inside the compaction
/// pipeline, which runs only requests that parsed compaction state —
/// [`run_completion`] routes every other request to the plain path — so the
/// predicate answers on that entry precondition instead of re-checking it.
///
/// # Panics
///
/// Panics if the request carries no compaction state: the pipeline's entry
/// contract guarantees the state, so a violation is a fatal bug.
fn is_threshold_control(parsed: &ParsedRequest) -> bool {
    #[expect(
        clippy::expect_used,
        reason = "the compaction pipeline runs only requests carrying compaction state"
    )]
    let compaction = parsed
        .compaction
        .as_ref()
        .expect("the compaction pipeline runs only requests carrying compaction state");

    matches!(
        compaction.control,
        Some(CompactionControl::MessagesThreshold { .. })
    )
}

/// The stable per-request render facts of one compaction pipeline request:
/// decided once at the surface boundary and consumed unchanged by every
/// outcome rendering.
pub(super) struct RenderContext {
    /// Whether the client requested a streaming response.
    pub(super) is_streaming: bool,

    /// Whether the client requested the trailing usage chunk; the renderers of
    /// formats that always include usage ignore it.
    pub(super) should_include_usage: bool,

    /// The public model id the client addressed.
    pub(super) public_id: String,

    /// The log-line context naming the operation, model, and provider.
    pub(super) log_context: String,
}

/// One compaction pipeline attempt's shared state: the facts every executor
/// outcome rendering and the native-failure fallback recurse over unchanged.
struct Pipeline<'a> {
    /// The gateway context feeding dispatches and renderings.
    context: &'a Context,

    /// The authenticated compaction carrier preparation.
    preparation: &'a BoundaryPreparation,

    /// The compaction account selection advanced across the pipeline's attempts.
    selection: &'a mut gateway::CompactionSelection,

    /// The request's accounting guard owning the draft between attempts.
    accounting: &'a mut ApiLogGuard,

    /// The request's stable render facts.
    render: &'a mut RenderContext,
}

/// The checkpoint-continue outcome's continuation facts, carried from the
/// executor outcome into [`checkpoint_completion`] as one unit.
struct CheckpointContinuation {
    /// The checkpoint prefix rendering ahead of the continuation.
    prefix: CheckpointPrefix,

    /// The continuation request.
    ir: crate::ir::IrRequest,

    /// Operation headers required by a native checkpoint continuation.
    overlay_headers: Vec<crate::provider::definition::template::ParamRow>,

    /// Sealer-proven identity of the newly emitted checkpoint.
    artifact_id: String,
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
///
/// The pipeline is the one logging site for its final failure: an error outcome
/// is logged exactly once through `render.log_context` — the operation, model,
/// and currently resolved provider, kept current across quota advances — after
/// the accounting guard is finalized. Callers must not log the error again.
pub(super) async fn run_compaction_pipeline<TFormat>(
    context: &Context,
    parsed: ParsedRequest,
    preparation: BoundaryPreparation,
    mut selection: gateway::CompactionSelection,
    log: ApiLogDraft,
    mut render: RenderContext,
    operation: OperationKind,
) -> Result<Response, Error>
where
    TFormat: ClientFormat + 'static,
    TFormat::JsonResponse: Serialize,
{
    let mut guard = ApiLogGuard::new(context.api_log_store.clone(), log);
    let outcome = loop {
        let routing = selection.routing.clone();
        let outcome = run_compaction_pipeline_inner::<TFormat>(
            &mut Pipeline {
                context,
                preparation: &preparation,
                selection: &mut selection,
                accounting: &mut guard,
                render: &mut render,
            },
            &parsed,
            routing,
        )
        .await;
        if matches!(outcome, Err(Error::QuotaRejected { .. })) {
            match selection
                .advance_after_quota_rejection(context, guard.draft_mut())
                .await
            {
                Ok(true) => {
                    render.log_context = format!(
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
    if let Err(error) = &outcome {
        let (log_outcome, http_status) = error.accounting();
        guard.finalize(log_outcome, Some(http_status)).await;
        log_error(&render.log_context, error);
    }
    outcome
}

/// Resolves and renders one compaction attempt, with errors owned by the outer accounting guard.
async fn run_compaction_pipeline_inner<TFormat>(
    pipeline: &mut Pipeline<'_>,
    parsed: &crate::format::ParsedRequest,
    routing: crate::provider::ModelRouting,
) -> Result<Response, Error>
where
    TFormat: ClientFormat + 'static,
    TFormat::JsonResponse: Serialize,
{
    let surface = compaction_surface(parsed);

    // The ledger's billed totals stamp the accounting row for every outcome
    // that carries one; the executor has already recorded each dispatch into
    // it (README §6.8).
    let stamp_billed = |draft: &mut ApiLogDraft, ledger: &CompactionLedger| {
        draft.stamp_billed_usage(ledger.reported_billed_totals());
    };

    if surface == CompactionSurface::Messages {
        let outcome = executor::execute_messages_request(
            pipeline.context,
            &routing,
            parsed,
            pipeline.preparation,
            pipeline.accounting.draft_mut(),
            Some(&mut pipeline.selection.ownership),
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

                ordinary_completion::<TFormat>(pipeline, routing, ir, dispatch, parsed).await
            }

            MessagesOuterOutcome::CompactionOnly {
                block,
                artifact_id,
                iterations,
                ledger,
            } => {
                stamp_billed(pipeline.accounting.draft_mut(), &ledger);

                let affinity = pipeline.selection.accepted().map(|accepted| {
                    accepted.completed_prefix(
                        crate::format::anthropic::messages::completed_compaction_anchors(
                            &block,
                            artifact_id.as_deref(),
                        ),
                    )
                });
                compaction_only_messages_response(
                    pipeline,
                    &block,
                    &iterations,
                    is_threshold_control(parsed),
                    affinity,
                )
            }

            MessagesOuterOutcome::NativeNoSummary {
                aggregation,
                ledger,
            } => {
                stamp_billed(pipeline.accounting.draft_mut(), &ledger);

                let affinity = pipeline.selection.accepted();
                aggregated_artifact_response::<TFormat>(pipeline, &aggregation, affinity)
            }

            MessagesOuterOutcome::ContinueWithCheckpoint {
                prefix_blocks,
                artifact_id,
                ir,
                overlay_headers,
                iterations,
                ledger,
            } => {
                stamp_billed(pipeline.accounting.draft_mut(), &ledger);

                checkpoint_completion::<TFormat>(
                    pipeline,
                    routing,
                    CheckpointContinuation {
                        prefix: CheckpointPrefix::Messages(prefix_blocks, iterations),
                        ir,
                        overlay_headers,
                        artifact_id,
                    },
                )
                .await
            }
        }
    } else {
        let outcome = executor::execute_responses_request(
            pipeline.context,
            &routing,
            parsed,
            pipeline.preparation,
            pipeline.accounting.draft_mut(),
            Some(&mut pipeline.selection.ownership),
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

                ordinary_completion::<TFormat>(pipeline, routing, ir, dispatch, parsed).await
            }

            ResponsesOuterOutcome::CompactionOnly {
                items,
                artifact_id,
                usage,
                ledger,
            } => {
                stamp_billed(pipeline.accounting.draft_mut(), &ledger);

                let response = compaction_only_responses_response(pipeline, &items, usage)?;
                pipeline
                    .selection
                    .register_gateway_window(&artifact_id, &items);
                Ok(response)
            }

            ResponsesOuterOutcome::NativeLifecycle {
                aggregation,
                ledger,
            } => {
                stamp_billed(pipeline.accounting.draft_mut(), &ledger);
                let affinity = pipeline.selection.accepted();
                aggregated_artifact_response::<TFormat>(pipeline, &aggregation, affinity)
            }

            ResponsesOuterOutcome::ContinueWithCheckpoint {
                prefix_items,
                artifact_id,
                ir,
                overlay_headers,
                ledger,
            } => {
                stamp_billed(pipeline.accounting.draft_mut(), &ledger);

                checkpoint_completion::<TFormat>(
                    pipeline,
                    routing,
                    CheckpointContinuation {
                        prefix: CheckpointPrefix::Responses(prefix_items),
                        ir,
                        overlay_headers,
                        artifact_id,
                    },
                )
                .await
            }
        }
    }
}

/// The ordinary continuation of a compaction pipeline outcome: the prepared
/// request runs the existing execute-and-render path, carrying the compaction
/// dispatch extension the executor resolved (passthrough controls, replay
/// overlays — empty for a plain replay). A native-unsupported upstream
/// rejection falls back to the gateway summary on the same account and
/// recurses into the pipeline over the same parsed request.
async fn ordinary_completion<TFormat>(
    pipeline: &mut Pipeline<'_>,
    routing: crate::provider::ModelRouting,
    ir: crate::ir::IrRequest,
    dispatch: CompactionDispatch,
    parsed: &ParsedRequest,
) -> Result<Response, Error>
where
    TFormat: ClientFormat + 'static,
    TFormat::JsonResponse: Serialize,
{
    let is_streaming = pipeline.render.is_streaming;

    let events = gateway::execute(
        pipeline.context,
        routing.clone(),
        ir,
        is_streaming,
        pipeline.accounting.draft_mut(),
        Some(&dispatch),
        Some(&mut pipeline.selection.ownership),
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
            if crate::compaction::native::classify_failure(classifier, status, &message).is_some() {
                let mut fallback_routing = routing;
                if let Some(section) = &mut fallback_routing.provider.definition.payload.compaction
                {
                    section.strategy =
                        crate::provider::definition::compaction::CompactionStrategy::GatewaySummary;
                }
                return Box::pin(run_compaction_pipeline_inner::<TFormat>(
                    pipeline,
                    parsed,
                    fallback_routing,
                ))
                .await;
            }
            return Err(Error::Upstream { status, message });
        }
        Err(error) => return Err(error.into()),
    };

    if is_streaming {
        let is_threshold = is_threshold_control(parsed);
        let renderer: Box<dyn ClientSseRenderer + Send> = if is_threshold {
            Box::new(
                crate::format::anthropic::messages::SseRenderer::new(
                    pipeline.render.public_id.clone(),
                )
                .with_threshold_compaction(true),
            )
        } else {
            TFormat::sse_renderer(
                pipeline.render.should_include_usage,
                pipeline.render.public_id.clone(),
            )
        };
        Ok(sse_response(
            events,
            renderer,
            pipeline.context.api_log_store.clone(),
            pipeline.accounting.take(),
            pipeline.selection.accepted(),
        ))
    } else {
        detached_aggregation::<TFormat>(
            events,
            pipeline.render.public_id.clone(),
            pipeline.context.api_log_store.clone(),
            pipeline.accounting.take(),
            pipeline.render.log_context.clone(),
            AggregationRender::Ordinary,
            pipeline.selection.accepted(),
        )
        .await
    }
}

/// The checkpoint-continue outcome: the continuation runs the existing
/// execute-and-render path while the executor's checkpoint renders ahead of
/// it — prefix items on the Responses surface, prefix blocks beside the
/// lifecycle's vendor-shaped iteration entries on the Messages surface
/// (README §6.8).
///
/// # Errors
///
/// Returns the selected account's continuation dispatch or response failure.
///
/// # Panics
///
/// Panics if the executor's sealed single checkpoint lacks its replayable wire fields.
async fn checkpoint_completion<TFormat>(
    pipeline: &mut Pipeline<'_>,
    routing: crate::provider::ModelRouting,
    continuation: CheckpointContinuation,
) -> Result<Response, Error>
where
    TFormat: ClientFormat + 'static,
    TFormat::JsonResponse: Serialize,
{
    let CheckpointContinuation {
        prefix,
        ir,
        overlay_headers,
        artifact_id,
    } = continuation;

    let dispatch = CompactionDispatch {
        overlay_headers,
        ..CompactionDispatch::default()
    };
    let events = gateway::execute(
        pipeline.context,
        routing,
        ir,
        pipeline.render.is_streaming,
        pipeline.accounting.draft_mut(),
        Some(&dispatch),
        Some(&mut pipeline.selection.ownership),
    )
    .await?;

    let affinity = pipeline.selection.accepted().map(|accepted| {
        let (projector, anchors) = match &prefix {
            CheckpointPrefix::Responses(items) => {
                #[expect(
                    clippy::expect_used,
                    reason = "the executor emits a sealed replayable checkpoint"
                )]
                let projector =
                    crate::format::openai::response::gateway_checkpoint_projection(items)
                        .expect("a prepared Responses checkpoint has a canonical projection");
                let mut anchors = projector.completed_anchors();
                anchors.push(crate::affinity::Anchor::Artifact(
                    projector.gateway_artifact(&artifact_id),
                ));
                (projector, anchors)
            }
            CheckpointPrefix::Messages(blocks, _) => {
                #[expect(
                    clippy::expect_used,
                    reason = "the executor emits one sealed Messages checkpoint"
                )]
                let block = blocks
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
            }
        };
        accepted.completed_checkpoint(projector, anchors)
    });

    if pipeline.render.is_streaming {
        let renderer = match prefix {
            CheckpointPrefix::Responses(items) => {
                openai_sse_renderer_with_items(pipeline.render.public_id.clone(), items)
            }
            CheckpointPrefix::Messages(blocks, iterations) => anthropic_sse_renderer_with_blocks(
                pipeline.render.public_id.clone(),
                blocks,
                iterations,
            ),
        };

        Ok(sse_response(
            events,
            renderer,
            pipeline.context.api_log_store.clone(),
            pipeline.accounting.take(),
            affinity,
        ))
    } else {
        detached_aggregation::<TFormat>(
            events,
            pipeline.render.public_id.clone(),
            pipeline.context.api_log_store.clone(),
            pipeline.accounting.take(),
            pipeline.render.log_context.clone(),
            AggregationRender::WithPrefix { prefix },
            affinity,
        )
        .await
    }
}

/// Boxes the Responses streaming renderer carrying checkpoint prefix items.
fn openai_sse_renderer_with_items(
    public_id: String,
    prefix_items: Vec<crate::format::compaction_wire::Checkpoint>,
) -> Box<dyn ClientSseRenderer + Send> {
    Box::new(
        crate::format::openai::response::SseRenderer::new(public_id)
            .with_prefix_items(prefix_items),
    )
}

/// Boxes the Messages streaming renderer carrying checkpoint prefix blocks and
/// the lifecycle's iteration entries.
fn anthropic_sse_renderer_with_blocks(
    public_id: String,
    prefix_blocks: Vec<crate::format::compaction_wire::Checkpoint>,
    iterations: Vec<crate::ir::compaction::UsageIteration>,
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
    pipeline: &mut Pipeline<'_>,
    items: &[crate::format::compaction_wire::Checkpoint],
    usage: IrUsage,
) -> Result<Response, Error> {
    if pipeline.render.is_streaming {
        let frames =
            render_trigger_stream_frames(pipeline.render.public_id.clone(), items.to_vec(), usage);

        Ok(prerendered_sse_response(
            frames,
            pipeline.context.api_log_store.clone(),
            pipeline.accounting.take(),
        ))
    } else {
        let aggregation = IrAggregation {
            usage: Some(usage),
            ..IrAggregation::default()
        };

        let body = crate::format::openai::response::Format::render_json_response_with_prefix(
            &pipeline.render.public_id,
            &aggregation,
            items,
            &[],
        );

        Ok(finalize_buffered(
            pipeline.context.api_log_store.clone(),
            pipeline.accounting.take(),
            &body,
        ))
    }
}

/// Renders the Messages compact-only outcome: one compaction block with
/// `stop_reason: "compaction"` and the iteration usage (README §6.8).
fn compaction_only_messages_response(
    pipeline: &mut Pipeline<'_>,
    block: &crate::format::compaction_wire::Checkpoint,
    iterations: &[crate::ir::compaction::UsageIteration],
    is_threshold_compaction: bool,
    mut affinity: Option<crate::affinity::Accepted>,
) -> Result<Response, Error> {
    if pipeline.render.is_streaming {
        let renderer = crate::format::anthropic::messages::SseRenderer::with_compaction_prefix(
            pipeline.render.public_id.clone(),
            vec![block.clone()],
            Some(String::from("compaction")),
        )
        .with_usage_iterations(iterations.to_vec())
        .with_threshold_compaction(is_threshold_compaction);

        let events = futures::stream::iter(vec![
            Ok::<IrEvent, gateway::Error>(IrEvent::MessageStart {
                id: pipeline.render.public_id.clone(),
                model: pipeline.render.public_id.clone(),
            }),
            Ok(IrEvent::Done),
        ]);

        Ok(sse_response(
            Box::pin(events),
            Box::new(renderer),
            pipeline.context.api_log_store.clone(),
            pipeline.accounting.take(),
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

        let body =
            render_compaction_only_response(&pipeline.render.public_id, block.clone(), &usage);

        if let Some(affinity) = affinity.as_mut() {
            affinity.complete_artifacts();
        }
        Ok(finalize_buffered(
            pipeline.context.api_log_store.clone(),
            pipeline.accounting.take(),
            &body,
        ))
    }
}

/// Renders a lifecycle the executor validated as one complete internal
/// artifact — a native trigger passthrough or a native no-summary outcome —
/// from its aggregation: the same body streaming and JSON callers see,
/// rendered through the request's own client format.
fn aggregated_artifact_response<TFormat>(
    pipeline: &mut Pipeline<'_>,
    aggregation: &IrAggregation,
    mut affinity: Option<crate::affinity::Accepted>,
) -> Result<Response, Error>
where
    TFormat: ClientFormat,
    TFormat::JsonResponse: Serialize,
{
    let mut log = pipeline.accounting.take();
    let public_id = pipeline.render.public_id.clone();

    if pipeline.render.is_streaming {
        // This same native observation is already present in the ledger; the streamed
        // events own its final observation instead, so it must not be added twice.
        log.billed_usage = None;
        let events = aggregation_events(aggregation, &public_id);

        let stream: IrEventStream = Box::pin(futures::stream::iter(
            events.into_iter().map(Ok::<IrEvent, gateway::Error>),
        ));

        Ok(sse_response(
            stream,
            TFormat::sse_renderer(pipeline.render.should_include_usage, public_id),
            pipeline.context.api_log_store.clone(),
            log,
            affinity,
        ))
    } else {
        // A validated single-artifact lifecycle carries no checkpoint prefix:
        // the plain aggregated render keeps its native semantics — usage
        // without gateway-fabricated iteration entries.
        let body = TFormat::render_json_response(&public_id, aggregation);
        if let Some(affinity) = &mut affinity {
            complete_accepted_json::<TFormat>(affinity, &body);
        }
        Ok(finalize_buffered(
            pipeline.context.api_log_store.clone(),
            log,
            &body,
        ))
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
    let log = ApiLogDraft::predispatch(
        api_key_id,
        routing.provider.id,
        routing.model.id,
        routing.model_route_id,
        ApiLogEndpointType::OpenaiResponsesCompact,
        false,
    );

    let mut guard = ApiLogGuard::new(context.api_log_store.clone(), log);
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
    match outcome {
        Ok(body) => {
            selection.register_compacted(&body);
            Ok(finalize_buffered(
                context.api_log_store.clone(),
                guard.take(),
                &body,
            ))
        }

        Err(error) => {
            let (log_outcome, http_status) = error.accounting();
            guard.finalize(log_outcome, Some(http_status)).await;
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
