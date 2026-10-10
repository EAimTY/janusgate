//! The compaction executor: strategy selection and multi-call coordination for
//! compaction operations (`docs/design/compaction.md`'s decision order).
//!
//! One entry point per client surface, each resolving the decision order for
//! its request: consume the boundary authenticated before account selection, check its
//! compatibility with the selected target and restore the active window, then — for a carrying
//! control — prefer the exact native mechanism, then a compatible native adapter, and
//! finally gateway summary inference. A dispatched native operation that
//! reports a positively classified unsupported boundary before any artifact is
//! committed falls back to summary inference exactly once; every other error
//! keeps its existing upstream semantics.
//!
//! Unknown support never probes upstreams: it selects gateway summarization
//! directly. The gateway-summary strategy skips native selection entirely for
//! new compaction work, while existing native state still replays through the
//! ordinary path. All internal dispatches run inside the handler future — one
//! cancellation scope, no fresh deadline per call — and every actual dispatch
//! records its usage into the request's ledger.

use super::{
    error::CompactionError,
    history::{
        self, Budgets, CheckpointInstructions, GatewayCheckpointPlan, RestoredBoundary,
        seal_gateway_checkpoint, seal_native_window,
    },
    native, summary,
    threshold::{
        MESSAGES_DEFAULT_THRESHOLD, estimate_context_tokens, responses_gateway_default_threshold,
    },
};
use crate::{
    api::Error as ApiError,
    compaction_model::{
        accounting::CompactionLedger,
        envelope::{
            EnvelopePayload, NativeReplayWindow, NativeWindowPayload, PrefixIdentity,
            ReplacementMode, summary_message,
        },
        mechanism::{Mechanism, MechanismSupport},
    },
    context::Context,
    format::{
        CompactionControl, NativeArtifact, ParsedRequest, UpstreamFormat,
        anthropic::messages::{GATEWAY_CARRIER_DESCRIPTION, gateway_carrier_block},
        compaction_control,
        openai::response::{
            compaction_item, render_compacted_response, render_gateway_compacted_response,
        },
    },
    gateway::{self, Error as GatewayError},
    ir::{IrAggregation, IrFinishReason, IrRequest, IrUsage},
    provider::{
        ModelRouting,
        definition::{
            compaction::{CompactionSection, CompactionStrategy, UnsupportedClassifier},
            template::{ParamRow, ValueExpr},
        },
    },
    storage::api_logs::ApiLogDraft,
};
use std::future::Future;

/// The shipped default summary-output budget (`docs/design/providers.md`): applied when the
/// definition carries no compaction section to state its own.
const DEFAULT_SUMMARY_BUDGET_TOKENS: u64 = 4096;

/// The shipped default retained-input target (`docs/design/providers.md`): applied when the
/// definition carries no compaction section to state its own.
const DEFAULT_RETENTION_BUDGET_TOKENS: u64 = 8192;

/// Converts a compaction failure into the shared API error taxonomy: the
/// compaction domain maps onto the gateway error domain in one place
/// ([`CompactionError`]'s `From` impl), and the API taxonomy converts the
/// gateway domain in its own single place.
fn api_error(error: CompactionError) -> ApiError {
    ApiError::from(GatewayError::from(error))
}

/// The header rows an ordinary replay or native passthrough dispatch carries
/// upstream so a replayed Anthropic compaction block stays acceptable to its
/// producer: a Claude on-demand block replays under the on-demand beta, a
/// threshold block under the threshold beta (the declaration's own contract —
/// the replay of a returned compaction block needs the operation's headers
/// wherever the block travels). Every other target and boundary carries no
/// overlay.
fn replay_overlay_headers(
    section: Option<&CompactionSection>,
    format: UpstreamFormat,
    boundary: Option<&RestoredBoundary>,
) -> Vec<ParamRow> {
    if format != UpstreamFormat::AnthropicMessages {
        return Vec::new();
    }

    let is_on_demand_block = match boundary {
        // A wrapped Anthropic on-demand block restored from a gateway carrier.
        Some(RestoredBoundary::Gateway { envelope }) => matches!(
            &envelope.payload,
            EnvelopePayload::NativeWindow(payload)
                if matches!(
                    payload.window,
                    NativeReplayWindow::AnthropicOnDemandBlock { .. }
                )
        ),

        // A native on-demand (leading) block replayed in place.
        Some(RestoredBoundary::Native(NativeArtifact::MessagesCompactionBlock {
            placement: crate::format::BoundaryPlacement::Leading,
            ..
        })) => true,

        _ => false,
    };

    let is_threshold_block = matches!(
        boundary,
        Some(RestoredBoundary::Native(
            NativeArtifact::MessagesCompactionBlock {
                placement: crate::format::BoundaryPlacement::Historical,
                ..
            }
        ))
    );

    let overlay = |rows: Option<&[ParamRow]>| rows.unwrap_or(&[]).to_vec();

    let mechanisms = section.map(|section| &section.mechanisms);

    if is_on_demand_block {
        let on_demand = mechanisms.and_then(|mechanisms| {
            mechanisms
                .messages_on_demand_compaction
                .as_ref()
                .map(|declaration| declaration.overlay_headers.as_deref())
        });

        overlay(on_demand.flatten())
    } else if is_threshold_block {
        let threshold = mechanisms.and_then(|mechanisms| {
            mechanisms
                .messages_threshold_compaction
                .as_ref()
                .map(|declaration| declaration.overlay_headers.as_deref())
        });

        overlay(threshold.flatten())
    } else {
        Vec::new()
    }
}

/// Combines two overlay-header declarations into one row set for one dispatch
/// (`docs/design/compaction.md`: the same headers serve the replay of a returned compaction
/// block, wherever the block later travels). The merge rule: a row whose name
/// both sides carry joins its two values into one comma-separated row — the
/// vocabulary multi-valued headers such as `anthropic-beta` define — keeping
/// the operation's row in place with its own value first and the replayed
/// state's value appended; a row only one side carries keeps its own row in
/// its side's order.
///
/// The join is deliberate, not list concatenation: the dispatch replaces
/// same-named rows by name ([`gateway::overlay_header_rows`]) and each header
/// insert keeps the last value, so two separate `anthropic-beta` rows would
/// silently reduce to one beta and drop the other mechanism's replay
/// requirement. When one side carries several same-named rows, the value
/// appends to the last such row, matching that last-insert-wins evaluation.
fn combine_overlay_rows(operation: Vec<ParamRow>, replay: Vec<ParamRow>) -> Vec<ParamRow> {
    let mut rows = operation;

    for row in replay {
        match rows
            .iter_mut()
            .rev()
            .find(|existing| existing.name.eq_ignore_ascii_case(&row.name))
        {
            Some(existing) => {
                existing.value.0.push(ValueExpr::Literal {
                    value: ",".to_string(),
                });
                existing.value.0.extend(row.value.0);
            }

            None => rows.push(row),
        }
    }

    rows
}

/// The definition's effective support of one mechanism under the routing's
/// model facts: the administrator override, then the discovered fact, then the
/// definition's declared default; nothing known resolves to unknown, which
/// selects gateway summarization without a speculative native call.
fn effective_support(
    routing: &ModelRouting,
    section: Option<&CompactionSection>,
    mechanism: Mechanism,
) -> MechanismSupport {
    routing
        .model
        .effective_mechanism_support(section)
        .find(|(candidate, _)| *candidate == mechanism)
        .map_or(MechanismSupport::Unknown, |(_mechanism, support)| support)
}

/// The gateway budgets of one routed request (`docs/design/providers.md` and
/// `docs/design/compaction.md`): the definition's stated budgets, or the shipped defaults when the
/// definition carries no compaction section.
fn budgets(section: Option<&CompactionSection>) -> Budgets {
    match section {
        Some(section) => Budgets {
            summary: section.summary_budget.get(),
            retention: section.retention_budget.get(),
        },

        None => Budgets {
            summary: DEFAULT_SUMMARY_BUDGET_TOKENS,
            retention: DEFAULT_RETENTION_BUDGET_TOKENS,
        },
    }
}

/// The summary output budget bounded by a compact-only caller cap: the smaller
/// of the configured summary budget and the caller's stated output limit.
fn summary_output_limit(section: Option<&CompactionSection>, caller_cap: Option<u64>) -> u64 {
    match caller_cap {
        Some(cap) => budgets(section).summary.min(cap),

        None => budgets(section).summary,
    }
}

/// Builds the native window payload of one wrapped native result: the routed
/// provider instance and upstream model as producer provenance, the producing
/// mechanism and format, and the exact native replay window.
fn native_window_payload(
    routing: &ModelRouting,
    mechanism: Mechanism,
    window: NativeReplayWindow,
    required_prefix_identity: PrefixIdentity,
) -> NativeWindowPayload {
    NativeWindowPayload {
        retained_messages: Vec::new(),
        retained_state_items: Vec::new(),
        producer_instance_id: NativeWindowPayload::producer_instance(routing.provider.id),
        upstream_model_id: routing.model.upstream_id.clone(),
        mechanism,
        native_format: routing.provider.definition.payload.format,
        required_prefix_identity: Some(required_prefix_identity),
        window,
    }
}

/// Converts billed sampling observations to concrete Messages iteration reports.
fn compaction_iterations(ledger: &CompactionLedger) -> Vec<crate::ir::compaction::UsageIteration> {
    use crate::ir::compaction::{SamplingKind, UsageIteration};
    ledger
        .iterations()
        .iter()
        .map(|usage| UsageIteration::gateway(SamplingKind::Compaction, *usage))
        .collect()
}

/// Whether one mechanism enters the native decision order for this request:
/// the strategy prefers native, the mechanism resolves to supported, and the
/// definition speaks the wire format the mechanism dispatches on.
fn is_native_eligible(
    routing: &ModelRouting,
    section: Option<&CompactionSection>,
    mechanism: Mechanism,
    format: UpstreamFormat,
) -> bool {
    section.is_none_or(|section| section.strategy == CompactionStrategy::NativePreferred)
        && effective_support(routing, section, mechanism) == MechanismSupport::Supported
        && routing.provider.definition.payload.format == format
}

/// One native stage of a decision order.
enum NativeAttempt<T> {
    /// The gate never opened: the mechanism is not eligible for this request,
    /// and nothing was dispatched.
    NotEligible,

    /// The dispatch committed its artifact.
    Committed(T),

    /// The upstream positively classified the mechanism unsupported — the one
    /// runtime evidence that permits the single summary fallback, and a
    /// fall-through to the next mechanism of the decision order. The
    /// classification reason is logged where it is observed.
    Unsupported,

    /// Any other failure: it keeps its existing upstream semantics, and
    /// propagates unchanged.
    Failed(GatewayError),
}

/// Runs one native stage of a decision order: owns the gate, the dispatch, and
/// the outcome classification. A closed gate (`None`) dispatches nothing —
/// the dispatch future is lazy, so no work is ever started for an ineligible
/// mechanism. A positively classified unsupported boundary falls through to
/// the next mechanism of the decision order; every other failure propagates
/// unchanged.
async fn try_native<T>(
    dispatch: Option<impl Future<Output = Result<T, native::NativeAttemptFailure>>>,
) -> NativeAttempt<T> {
    let Some(dispatch) = dispatch else {
        return NativeAttempt::NotEligible;
    };

    match dispatch.await {
        Ok(artifact) => NativeAttempt::Committed(artifact),

        Err(native::NativeAttemptFailure::ClassifiedUnsupported(reason)) => {
            log::debug!(
                "compaction: the native operation was classified unsupported ({reason}); \
                 falling through the decision order"
            );

            NativeAttempt::Unsupported
        }

        Err(native::NativeAttemptFailure::Failed(error)) => NativeAttempt::Failed(error),
    }
}

/// Dispatches the native explicit-trigger passthrough: the trigger rides the
/// ordinary request upstream, and the mechanism's classifier decides between
/// the fall-through evidence and the upstream failure.
async fn dispatch_native_trigger(
    context: &Context,
    routing: &ModelRouting,
    mut passthrough: IrRequest,
    classifier: Option<&UnsupportedClassifier>,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
    ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<IrAggregation, native::NativeAttemptFailure> {
    passthrough.has_compaction_trigger = true;

    match gateway::execute_internal(context, routing, passthrough, ledger, log, ownership).await {
        Ok(aggregation) => Ok(aggregation),

        Err(GatewayError::Upstream { status, detail }) => {
            // The error carries the complete bounded error body the ordinary dispatch boundary
            // retained, so the classifier probes the same whole-body evidence the native
            // operations classify on; the excerpt is a render-side concern only.
            if let Some(reason) = native::classify_failure(classifier, status, &detail) {
                return Err(native::NativeAttemptFailure::ClassifiedUnsupported(reason));
            }

            Err(native::NativeAttemptFailure::Failed(
                GatewayError::Upstream { status, detail },
            ))
        }

        Err(error) => Err(native::NativeAttemptFailure::Failed(error)),
    }
}

/// Validates the native trigger lifecycle: exactly one completed compaction
/// item under a normal finish, before any outer success is declared.
fn validate_trigger_aggregation(
    aggregation: IrAggregation,
) -> Result<IrAggregation, CompactionError> {
    if aggregation.compaction_states.len() != 1 || aggregation.finish != Some(IrFinishReason::Stop)
    {
        return Err(CompactionError::inaccessible_native_state(
            "the native trigger response did not carry exactly one completed compaction item",
        ));
    }

    Ok(aggregation)
}

/// One completed gateway checkpoint and the metadata already authenticated during sealing.
struct GatewaySummaryCheckpoint {
    /// Exact retained-window and instruction-overlap plan.
    plan: GatewayCheckpointPlan,
    /// Complete validated summary used by normal response/continuation rendering.
    summary_text: String,
    /// Actual sealed outward carrier.
    carrier: String,
    /// Existing authenticated envelope identity, never an added routing token.
    artifact_id: String,
}

/// Runs the gateway summary flow end to end: plans the checkpoint from the
/// restored window, dispatches one internal summary inference, validates the
/// summary, and seals the checkpoint envelope into its carrier. The summary
/// call's usage rides the ledger.
///
/// Returns the plan (the continuation's building block), the summary text (the
/// Messages carrier's visible content), the sealed carrier, and its authenticated artifact ID.
///
/// # Errors
///
/// Returns [`CompactionError`] for an unplannable window, a dispatch failure,
/// and an unusable summary; sealing failures propagate as configured.
#[expect(
    clippy::too_many_arguments,
    reason = "the flow's inputs are the plan, the dispatch, and the carrier facts the \
              decision order resolves together; splitting them would scatter the order"
)]
async fn run_gateway_summary(
    context: &Context,
    routing: &ModelRouting,
    restored: &IrRequest,
    instructions: &CheckpointInstructions<'_>,
    is_mandatory_active_turn: bool,
    compaction_instructions: Option<&str>,
    output_cap: Option<u64>,
    replacement: ReplacementMode,
    binds_visible_content: bool,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
    ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<GatewaySummaryCheckpoint, CompactionError> {
    let mut plan = history::plan_gateway_checkpoint(
        restored,
        instructions,
        is_mandatory_active_turn,
        budgets(routing.provider.definition.payload.compaction.as_ref()),
    )?;

    let summarizer =
        summary::plan_summary_request(restored, &mut plan, compaction_instructions, output_cap);

    let aggregation =
        gateway::execute_internal(context, routing, summarizer, ledger, log, ownership)
            .await
            .map_err(CompactionError::Upstream)?;

    let summary_text = summary::validate_summary(&aggregation)?;

    let (envelope, carrier) = seal_gateway_checkpoint(
        &plan,
        summary_text.clone(),
        replacement,
        binds_visible_content.then_some(summary_text.as_str()),
    )?;

    Ok(GatewaySummaryCheckpoint {
        plan,
        summary_text,
        carrier,
        artifact_id: envelope.artifact_id,
    })
}

/// A native threshold adapter result: checkpoint carrier and the prepared continuation.
struct NativeCheckpoint {
    /// Authenticated identity captured when the outward carrier is sealed.
    artifact_id: String,
    /// Authenticated outward transport state.
    carrier: String,
    /// Visible Messages carrier content, bound to the envelope.
    visible_content: Option<String>,
    /// Exact native window followed by the whole pending interaction.
    continuation: IrRequest,
    /// Headers required when the native state replays.
    overlay_headers: Vec<ParamRow>,
}

/// Uses a declared explicit native operation to compact the older prefix of an emulated threshold.
/// Classifier-positive unsupported responses return to the caller's single gateway fallback.
///
/// # Panics
///
/// Panics if a selected native operation lacks its required declaration, an invariant
/// established by the selection conditions.
#[allow(clippy::too_many_arguments)]
async fn native_threshold_checkpoint(
    context: &Context,
    routing: &ModelRouting,
    ir: &IrRequest,
    plan_instructions: &CheckpointInstructions<'_>,
    instructions: Option<&str>,
    is_messages_surface: bool,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
    mut ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<Option<NativeCheckpoint>, CompactionError> {
    let definition = &routing.provider.definition.payload;
    let section = definition.compaction.as_ref();
    if section.is_some_and(|section| section.strategy != CompactionStrategy::NativePreferred) {
        return Ok(None);
    }
    let standalone = section
        .and_then(|section| section.mechanisms.responses_standalone_compact.as_ref())
        .and_then(|declaration| declaration.operation.as_ref());
    let on_demand =
        section.and_then(|section| section.mechanisms.messages_on_demand_compaction.as_ref());

    // One eligible checkpoint adapter, in the decision order's adapter
    // sequence: the standalone operation, the explicit trigger, the on-demand
    // operation.
    let mechanism = [
        (
            Mechanism::ResponsesStandaloneCompact,
            standalone.is_some(),
            UpstreamFormat::OpenAiResponse,
        ),
        (
            Mechanism::ResponsesExplicitTrigger,
            true,
            UpstreamFormat::OpenAiResponse,
        ),
        (
            Mechanism::MessagesOnDemandCompaction,
            on_demand.is_some(),
            UpstreamFormat::AnthropicMessages,
        ),
    ]
    .into_iter()
    .find(|(mechanism, has_declaration, format)| {
        is_native_eligible(routing, section, *mechanism, *format) && *has_declaration
    })
    .map(|(mechanism, _has_declaration, _format)| mechanism);

    let Some(mechanism) = mechanism else {
        return Ok(None);
    };

    let mut plan = history::plan_native_checkpoint(ir, plan_instructions, budgets(section))?;

    // The adapter dispatches the summary input alone, never a clone of the
    // full history: the prefix is built field by field from the borrowed
    // request, riding only the native state above its boundary.
    let summary_input = std::mem::take(&mut plan.summary_input_messages);

    let prefix_boundary = summary_input.len();

    let prefix = IrRequest {
        has_compaction_trigger: ir.has_compaction_trigger,
        model: ir.model.clone(),
        instructions: ir.instructions.clone(),
        messages: summary_input,
        tools: ir.tools.clone(),
        tool_choice: ir.tool_choice.clone(),
        temperature: ir.temperature,
        top_p: ir.top_p,
        max_output_tokens: ir.max_output_tokens,
        response_format: ir.response_format.clone(),
        reasoning_effort: ir.reasoning_effort,
        state_items: ir
            .state_items
            .iter()
            .filter(|item| item.offset < prefix_boundary)
            .cloned()
            .collect(),
    };

    let mut overlay_headers = Vec::new();
    let attempt = try_native(Some(async {
        match mechanism {
            Mechanism::ResponsesStandaloneCompact => {
                #[expect(
                    clippy::expect_used,
                    reason = "selection requires the standalone operation"
                )]
                let operation = standalone.expect("selected standalone operation");
                native::responses_standalone(
                    context,
                    routing,
                    operation,
                    &prefix,
                    ledger,
                    log,
                    ownership.as_deref_mut(),
                )
                .await
                .map(|native| NativeReplayWindow::ResponsesWindow {
                    items: native.output,
                })
            }
            Mechanism::ResponsesExplicitTrigger => {
                let classifier = section
                    .and_then(|section| section.mechanisms.responses_explicit_trigger.as_ref())
                    .and_then(|declaration| declaration.unsupported.as_ref());
                dispatch_native_trigger(
                    context,
                    routing,
                    prefix,
                    classifier,
                    ledger,
                    log,
                    ownership.as_deref_mut(),
                )
                .await
                .and_then(|aggregation| {
                    validate_trigger_aggregation(aggregation)
                        .map_err(native::NativeAttemptFailure::from)
                })
                .map(|aggregation| NativeReplayWindow::ResponsesWindow {
                    items: aggregation.compaction_states,
                })
            }
            Mechanism::MessagesOnDemandCompaction => {
                #[expect(
                    clippy::expect_used,
                    reason = "selection requires the on-demand declaration"
                )]
                let declaration = on_demand.expect("selected on-demand declaration");
                overlay_headers = declaration.overlay_headers.clone().unwrap_or_default();
                match native::messages_on_demand(
                    context,
                    routing,
                    &overlay_headers,
                    declaration.unsupported.as_ref(),
                    &prefix,
                    instructions,
                    summary_output_limit(section, None),
                    ledger,
                    log,
                    ownership,
                )
                .await
                {
                    Ok(native::OnDemandOutcome::Success { block, .. }) => {
                        Ok(NativeReplayWindow::AnthropicOnDemandBlock { block })
                    }
                    // The threshold adapter cannot continue from a no-summary
                    // outcome: it fails explicitly instead of rendering a
                    // checkpoint without its required native window.
                    Ok(native::OnDemandOutcome::NoSummary { .. }) => {
                        Err(native::NativeAttemptFailure::from(
                            CompactionError::inaccessible_native_state(
                                "the threshold adapter returned no usable native checkpoint",
                            ),
                        ))
                    }
                    Err(failure) => Err(failure),
                }
            }
            // The adapter selection above admits only the three checkpoint
            // mechanisms; the passthrough mechanisms never dispatch here.
            #[expect(
                clippy::unreachable,
                reason = "the adapter selection admits only the three checkpoint mechanisms"
            )]
            Mechanism::ResponsesAutomaticCompaction | Mechanism::MessagesThresholdCompaction => {
                unreachable!("a passthrough mechanism is never selected for a checkpoint")
            }
        }
    }))
    .await;
    let window = match attempt {
        NativeAttempt::Committed(window) => window,
        // Not eligible cannot occur — the gate opened to dispatch — and a
        // classified unsupported boundary falls to the gateway summary.
        NativeAttempt::Unsupported | NativeAttempt::NotEligible => return Ok(None),
        NativeAttempt::Failed(error) => return Err(CompactionError::Upstream(error)),
    };
    let mut payload = native_window_payload(
        routing,
        mechanism,
        window,
        history::produced_prefix_identity(ir),
    );
    payload.retained_messages = plan.retained_units.into_iter().flatten().collect();
    payload.retained_state_items = plan
        .retained_state_items
        .into_iter()
        .map(|mut item| {
            item.offset -= 1;
            item
        })
        .collect();
    let continuation = history::native_continuation(ir, &payload)?;
    let visible_content = is_messages_surface.then(|| GATEWAY_CARRIER_DESCRIPTION.to_string());
    let (envelope, carrier) = seal_native_window(
        payload,
        ReplacementMode::HistoricalCheckpoint,
        visible_content.as_deref(),
    )?;
    Ok(Some(NativeCheckpoint {
        artifact_id: envelope.artifact_id,
        carrier,
        visible_content,
        continuation,
        overlay_headers,
    }))
}

/// Builds the continuation request of a completed checkpoint: the restored
/// compacted window — the summary message, the retained units, and the native
/// state riding them — with the caller's instruction context and output budget
/// preserved and every compaction control stripped. Built field by field from
/// the borrowed window, so the covered history and state are never cloned just
/// to be overwritten.
#[must_use]
fn continuation_request(
    restored: &IrRequest,
    summary_text: &str,
    plan: &GatewayCheckpointPlan,
) -> IrRequest {
    let mut messages = Vec::with_capacity(1 + plan.retained_units.len());

    messages.push(summary_message(summary_text));

    for unit_messages in &plan.retained_units {
        messages.extend(unit_messages.iter().cloned());
    }

    IrRequest {
        has_compaction_trigger: restored.has_compaction_trigger,
        model: restored.model.clone(),
        instructions: restored.instructions.clone(),
        messages,
        tools: restored.tools.clone(),
        tool_choice: restored.tool_choice.clone(),
        temperature: restored.temperature,
        top_p: restored.top_p,
        max_output_tokens: restored.max_output_tokens,
        response_format: restored.response_format.clone(),
        reasoning_effort: restored.reasoning_effort,
        state_items: plan.retained_state_items.clone(),
    }
}

/// The outer outcome of a Responses completion request the executor resolved.
#[derive(Debug)]
pub(crate) enum ResponsesOuterOutcome {
    /// The request continues through the existing pipeline unchanged: a pure
    /// replay, a below-threshold generation, or a native automatic passthrough.
    /// `context_management` carries the vendor-shaped automatic controls a
    /// native automatic upstream receives, rendered through the extension
    /// entry point of the Responses body render; `overlay_headers` carries the
    /// operation headers a replayed native compaction state requires.
    Ordinary {
        /// The prepared request.
        ir: IrRequest,

        /// The automatic compaction controls, when passthrough carries them.
        context_management: Option<Vec<crate::ir::compaction::AutomaticEntry>>,

        /// The header rows the replayed native state's operation requires.
        overlay_headers: Vec<ParamRow>,
    },

    /// The synthesized compact-only artifact: the checkpoint items the caller's
    /// lifecycle renders — one gateway carrier or one wrapped-native carrier —
    /// beside the billed usage of the participating calls.
    CompactionOnly {
        /// The compaction output items, in output order.
        items: Vec<crate::format::compaction_wire::Checkpoint>,

        /// Sealer-proven identity of the newly emitted gateway carrier, never serialized.
        artifact_id: String,

        /// The billed usage of the participating calls.
        usage: IrUsage,

        /// The request's ledger.
        ledger: CompactionLedger,
    },

    /// A native explicit trigger passthrough: the complete upstream lifecycle,
    /// validated to carry exactly one completed compaction item before any
    /// outer success is declared.
    NativeLifecycle {
        /// The aggregated upstream response.
        aggregation: IrAggregation,

        /// The request's ledger.
        ledger: CompactionLedger,
    },

    /// An emulated threshold compaction with continuation: the checkpoint items
    /// render before the answer, and the carried request is the continuation —
    /// the restored window, controls stripped, original output budget.
    ContinueWithCheckpoint {
        /// The checkpoint items rendering ahead of the answer.
        prefix_items: Vec<crate::format::compaction_wire::Checkpoint>,

        /// Sealer-proven identity of the newly emitted checkpoint.
        artifact_id: String,

        /// The continuation request.
        ir: IrRequest,

        /// Operation headers required by a native checkpoint continuation.
        overlay_headers: Vec<ParamRow>,

        /// The request's ledger.
        ledger: CompactionLedger,
    },
}

/// The outer outcome of a Messages completion request the executor resolved.
#[derive(Debug)]
pub(crate) enum MessagesOuterOutcome {
    /// The request continues through the existing pipeline unchanged: a pure
    /// replay or a below-threshold generation. `threshold_edits` carries the
    /// vendor-shaped edits a native threshold upstream receives, rendered
    /// through the threshold body render with the definition's overlay headers;
    /// `overlay_headers` carries the operation headers a replayed native
    /// compaction state — or the native threshold passthrough itself — requires.
    Ordinary {
        /// The prepared request.
        ir: IrRequest,

        /// The threshold edits, when passthrough carries them.
        threshold_edits: Option<Vec<crate::ir::compaction::ThresholdEdit>>,

        /// The header rows the replayed native state's operation requires.
        overlay_headers: Vec<ParamRow>,
    },

    /// The compact-only artifact — a native compaction block or a gateway
    /// carrier block — with `stop_reason: "compaction"` and no answer.
    CompactionOnly {
        /// The compaction block.
        block: crate::format::compaction_wire::Checkpoint,

        /// Sealer-authenticated identity for gateway carriers; absent for raw native blocks.
        artifact_id: Option<String>,

        /// The vendor-shaped `usage.iterations` entries to render.
        iterations: Vec<crate::ir::compaction::UsageIteration>,

        /// The request's ledger.
        ledger: CompactionLedger,
    },

    /// The native on-demand operation returned its documented no-summary
    /// outcome: the original history stands, and the consumed native response
    /// renders with its own stop reason and usage. Neither an error nor a
    /// success artifact.
    NativeNoSummary {
        /// The aggregated native response.
        aggregation: IrAggregation,

        /// The request's ledger.
        ledger: CompactionLedger,
    },

    /// An emulated threshold compaction with continuation: the checkpoint
    /// blocks render before the answer, and the carried request is the
    /// continuation.
    ContinueWithCheckpoint {
        /// The checkpoint blocks rendering ahead of the answer.
        prefix_blocks: Vec<crate::format::compaction_wire::Checkpoint>,

        /// Sealer-proven identity of the newly emitted checkpoint.
        artifact_id: String,

        /// The continuation request.
        ir: IrRequest,

        /// Operation headers required by a native checkpoint continuation.
        overlay_headers: Vec<ParamRow>,

        /// The vendor-shaped `usage.iterations` entries to render.
        iterations: Vec<crate::ir::compaction::UsageIteration>,

        /// The request's ledger.
        ledger: CompactionLedger,
    },
}

/// Executes the Responses standalone compact operation for one parsed compact
/// request: restores the active window, dispatches the native standalone
/// operation when eligible, and otherwise plans and seals a gateway checkpoint
/// through one internal summary inference. The outcome renders the
/// `response.compaction` body — the canonical next window — whose billed usage
/// totals are stamped onto the accounting draft.
///
/// # Errors
///
/// Returns [`ApiError`] for every failure decision; the outcome is always the
/// rendered success body or the error, never a partial artifact.
pub(crate) async fn execute_responses_compact(
    context: &Context,
    routing: &ModelRouting,
    parsed: &ParsedRequest,
    preparation: &crate::compaction_model::boundary::BoundaryPreparation,
    log: &mut ApiLogDraft,
    mut ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<crate::format::openai::response::CompactedResponseBody, ApiError> {
    let compaction = parsed.compaction.as_ref().ok_or_else(|| {
        ApiError::from(GatewayError::Internal {
            message: "the compact route's parsed request carries no compaction facts".to_string(),
        })
    })?;

    let mut ir = parsed.ir.clone();

    let (_restored, instructions) =
        history::prepare_for_target(&mut ir, compaction, routing, preparation)
            .map_err(api_error)?;

    let section = routing.provider.definition.payload.compaction.as_ref();

    let mut ledger = CompactionLedger::default();

    // Native standalone first: eligible when the strategy prefers native, the
    // mechanism resolves to supported, the operation descriptor exists, and the
    // definition speaks the Responses wire the compact body renders.
    let operation = section
        .and_then(|section| section.mechanisms.responses_standalone_compact.as_ref())
        .and_then(|declaration| declaration.operation.as_ref());

    let native_window = try_native(
        operation
            .filter(|_operation| {
                is_native_eligible(
                    routing,
                    section,
                    Mechanism::ResponsesStandaloneCompact,
                    UpstreamFormat::OpenAiResponse,
                )
            })
            .map(|operation| async {
                native::responses_standalone(
                    context,
                    routing,
                    operation,
                    &ir,
                    &mut ledger,
                    log,
                    ownership.as_deref_mut(),
                )
                .await
            }),
    )
    .await;

    if let NativeAttempt::Committed(native) = native_window {
        // This flow's rendered body carries no ledger, so the accounting row
        // is stamped here; every ledger-carrying outcome is stamped by the
        // serving layer from the returned ledger instead (`docs/design/compaction.md`).
        log.stamp_billed_usage(ledger.reported_billed_totals());

        return Ok(render_compacted_response(native));
    }

    if let NativeAttempt::Failed(error) = native_window {
        return Err(ApiError::from(error));
    }

    // A classified unsupported boundary (or an ineligible gate) falls to the
    // gateway summary — the strategy's own choice, the unknown-support path,
    // or the single classifier-positive fallback. Compact-only: a final user
    // turn may be summarized, and the compact body carries no output cap.
    let GatewaySummaryCheckpoint {
        carrier,
        artifact_id,
        ..
    } = run_gateway_summary(
        context,
        routing,
        &ir,
        &instructions,
        false,
        None,
        None,
        ReplacementMode::CanonicalWindow,
        false,
        &mut ledger,
        log,
        ownership,
    )
    .await
    .map_err(api_error)?;

    let totals = ledger.billed_totals();

    log.stamp_billed_usage(ledger.reported_billed_totals());

    Ok(render_gateway_compacted_response(
        carrier,
        artifact_id,
        totals,
    ))
}

/// Executes a Responses completion request's compaction decision order and
/// returns the outer outcome the serving pipeline renders: the ordinary
/// pipeline for replays and passthroughs, the synthesized compact-only
/// lifecycles for trigger requests, and the checkpoint-plus-continuation shape
/// for emulated automatic thresholds.
///
/// # Errors
///
/// Returns [`ApiError`] for every failure decision.
pub(crate) async fn execute_responses_request(
    context: &Context,
    routing: &ModelRouting,
    parsed: &ParsedRequest,
    preparation: &crate::compaction_model::boundary::BoundaryPreparation,
    log: &mut ApiLogDraft,
    mut ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<ResponsesOuterOutcome, ApiError> {
    let format = routing.provider.definition.payload.format;

    let Some(compaction) = parsed.compaction.as_ref() else {
        return Ok(ResponsesOuterOutcome::Ordinary {
            ir: parsed.ir.clone(),
            context_management: None,
            overlay_headers: Vec::new(),
        });
    };

    let mut ir = parsed.ir.clone();

    let (restored, instructions) =
        history::prepare_for_target(&mut ir, compaction, routing, preparation)
            .map_err(api_error)?;

    let replay_overlay = replay_overlay_headers(
        routing.provider.definition.payload.compaction.as_ref(),
        format,
        restored.as_ref(),
    );

    let Some(control) = compaction.control.clone() else {
        // A pure replay: the restored window runs through the ordinary
        // pipeline, and replay adds no compaction cost.
        return Ok(ResponsesOuterOutcome::Ordinary {
            ir,
            context_management: None,
            overlay_headers: replay_overlay,
        });
    };

    let mut ledger = CompactionLedger::default();

    match control {
        CompactionControl::ResponsesStandalone => Err(ApiError::from(GatewayError::Internal {
            message: "the standalone compact control never reaches the completion endpoint"
                .to_string(),
        })),

        CompactionControl::ResponsesTrigger => responses_trigger_flow(
            context,
            routing,
            &ir,
            &instructions,
            &mut ledger,
            log,
            ownership.as_deref_mut(),
        )
        .await
        .map_err(api_error),

        CompactionControl::ResponsesAutomatic { entries } => responses_automatic_flow(
            context,
            routing,
            &ir,
            &instructions,
            &entries,
            replay_overlay,
            &mut ledger,
            log,
            ownership,
        )
        .await
        .map_err(api_error),

        CompactionControl::MessagesOnDemand { .. }
        | CompactionControl::MessagesThreshold { .. } => {
            Err(ApiError::from(GatewayError::Internal {
                message: "a Messages compaction control reached the Responses surface".to_string(),
            }))
        }
    }
}

/// Resolves the Responses explicit-trigger decision order: the exact native
/// trigger, then the standalone adapter, then the on-demand adapter, then
/// gateway summary — every outcome a compact-only lifecycle. A positively
/// classified unsupported boundary at any native stage falls through to the
/// next mechanism; every other failure propagates unchanged.
async fn responses_trigger_flow(
    context: &Context,
    routing: &ModelRouting,
    ir: &IrRequest,
    instructions: &CheckpointInstructions<'_>,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
    mut ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<ResponsesOuterOutcome, CompactionError> {
    let definition = &routing.provider.definition.payload;

    let section = definition.compaction.as_ref();

    // The exact native mechanism: the trigger rides the ordinary request
    // upstream, and the lifecycle must carry exactly one completed compaction
    // item before any outer success.
    let classifier = section
        .and_then(|section| section.mechanisms.responses_explicit_trigger.as_ref())
        .and_then(|declaration| declaration.unsupported.as_ref());

    let trigger = try_native(
        is_native_eligible(
            routing,
            section,
            Mechanism::ResponsesExplicitTrigger,
            UpstreamFormat::OpenAiResponse,
        )
        .then(|| {
            dispatch_native_trigger(
                context,
                routing,
                ir.clone(),
                classifier,
                ledger,
                log,
                ownership.as_deref_mut(),
            )
        }),
    )
    .await;

    if let NativeAttempt::Committed(aggregation) = trigger {
        let aggregation = validate_trigger_aggregation(aggregation)?;

        return Ok(ResponsesOuterOutcome::NativeLifecycle {
            aggregation,
            ledger: std::mem::take(ledger),
        });
    }

    if let NativeAttempt::Failed(error) = trigger {
        return Err(CompactionError::Upstream(error));
    }

    // The same-format adapter: the trigger compacts through the standalone
    // operation, whose canonical window seals into one carrier item.
    let operation = section
        .and_then(|section| section.mechanisms.responses_standalone_compact.as_ref())
        .and_then(|declaration| declaration.operation.as_ref());

    let standalone = try_native(
        operation
            .filter(|_operation| {
                is_native_eligible(
                    routing,
                    section,
                    Mechanism::ResponsesStandaloneCompact,
                    UpstreamFormat::OpenAiResponse,
                )
            })
            .map(|operation| async {
                native::responses_standalone(
                    context,
                    routing,
                    operation,
                    ir,
                    ledger,
                    log,
                    ownership.as_deref_mut(),
                )
                .await
            }),
    )
    .await;

    if let NativeAttempt::Committed(native) = standalone {
        let (envelope, carrier) = seal_native_window(
            native_window_payload(
                routing,
                Mechanism::ResponsesStandaloneCompact,
                NativeReplayWindow::ResponsesWindow {
                    items: native.output,
                },
                history::produced_prefix_identity(ir),
            ),
            ReplacementMode::CanonicalWindow,
            None,
        )?;

        return Ok(ResponsesOuterOutcome::CompactionOnly {
            items: vec![compaction_item(None, carrier, None)],
            artifact_id: envelope.artifact_id,
            usage: ledger.billed_totals(),
            ledger: std::mem::take(ledger),
        });
    }

    if let NativeAttempt::Failed(error) = standalone {
        return Err(CompactionError::Upstream(error));
    }

    // The cross-format adapter: the trigger compacts through the Messages
    // on-demand operation, whose block seals into one carrier item with the
    // canonical-window replacement the trigger contract demands. The
    // declaration's existence gates the stage, as in the on-demand flow: the
    // effective support may resolve from model facts or an override over a
    // declaration with no headers, and an absent overlay dispatches with none.
    let on_demand =
        section.and_then(|section| section.mechanisms.messages_on_demand_compaction.as_ref());

    let on_demand_attempt = try_native(
        on_demand
            .filter(|_declaration| {
                is_native_eligible(
                    routing,
                    section,
                    Mechanism::MessagesOnDemandCompaction,
                    UpstreamFormat::AnthropicMessages,
                )
            })
            .map(|declaration| async {
                // The adapter cannot satisfy the trigger's exactly-one-artifact
                // contract with a no-summary outcome, so the outcome fails
                // explicitly instead of rendering a successful lifecycle without
                // its required artifact.
                match native::messages_on_demand(
                    context,
                    routing,
                    declaration.overlay_headers.as_deref().unwrap_or(&[]),
                    declaration.unsupported.as_ref(),
                    ir,
                    None,
                    summary_output_limit(section, ir.max_output_tokens),
                    ledger,
                    log,
                    ownership.as_deref_mut(),
                )
                .await?
                {
                    native::OnDemandOutcome::Success { block, .. } => Ok(block),

                    native::OnDemandOutcome::NoSummary { .. } => {
                        Err(native::NativeAttemptFailure::from(
                            CompactionError::inaccessible_native_state(
                                "the native on-demand compaction returned no usable block, so the \
                         trigger contract cannot be satisfied",
                            ),
                        ))
                    }
                }
            }),
    )
    .await;

    if let NativeAttempt::Committed(block) = on_demand_attempt {
        let (envelope, carrier) = seal_native_window(
            native_window_payload(
                routing,
                Mechanism::MessagesOnDemandCompaction,
                NativeReplayWindow::AnthropicOnDemandBlock { block },
                history::produced_prefix_identity(ir),
            ),
            ReplacementMode::CanonicalWindow,
            None,
        )?;

        return Ok(ResponsesOuterOutcome::CompactionOnly {
            items: vec![compaction_item(None, carrier, None)],
            artifact_id: envelope.artifact_id,
            usage: ledger.billed_totals(),
            ledger: std::mem::take(ledger),
        });
    }

    if let NativeAttempt::Failed(error) = on_demand_attempt {
        return Err(CompactionError::Upstream(error));
    }

    // Gateway summary: the chain's final mechanism. The trigger is
    // compact-only, so the caller's output limit caps the summary.
    let GatewaySummaryCheckpoint {
        carrier,
        artifact_id,
        ..
    } = run_gateway_summary(
        context,
        routing,
        ir,
        instructions,
        false,
        None,
        ir.max_output_tokens,
        ReplacementMode::CanonicalWindow,
        false,
        ledger,
        log,
        ownership,
    )
    .await?;

    Ok(ResponsesOuterOutcome::CompactionOnly {
        items: vec![compaction_item(None, carrier, None)],
        artifact_id,
        usage: ledger.billed_totals(),
        ledger: std::mem::take(ledger),
    })
}

/// Resolves the Responses automatic decision order: native automatic
/// passthrough, else the gateway-emulated threshold workflow — measure once at
/// the request boundary over the restored window, compact at or above the
/// threshold, and continue on the compacted window.
#[expect(
    clippy::too_many_arguments,
    reason = "one same-account stage keeps its protocol controls, billing, and binding together"
)]
async fn responses_automatic_flow(
    context: &Context,
    routing: &ModelRouting,
    ir: &IrRequest,
    instructions: &CheckpointInstructions<'_>,
    entries: &[crate::ir::compaction::AutomaticEntry],
    replay_overlay: Vec<ParamRow>,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
    mut ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<ResponsesOuterOutcome, CompactionError> {
    let definition = &routing.provider.definition.payload;

    let section = definition.compaction.as_ref();

    // Native automatic: the controls pass through upstream verbatim, preserving
    // every compaction item and its position in the ordinary pipeline.
    if section.is_none_or(|section| section.strategy == CompactionStrategy::NativePreferred)
        && effective_support(routing, section, Mechanism::ResponsesAutomaticCompaction)
            == MechanismSupport::Supported
        && definition.format == UpstreamFormat::OpenAiResponse
    {
        return Ok(ResponsesOuterOutcome::Ordinary {
            ir: ir.clone(),
            context_management: Some(entries.to_vec()),
            // The automatic passthrough rides the Responses wire; the replay
            // overlay is empty for it by construction.
            overlay_headers: replay_overlay,
        });
    }

    // Gateway emulation: the first stated threshold drives the evaluation; a
    // request with none uses the documented capacity default, whose declared
    // capacity is required.
    let threshold = match entries.iter().find_map(|entry| entry.compact_threshold) {
        Some(threshold) => threshold,

        None => responses_gateway_default_threshold(
            &routing.public_id,
            effective_input_capacity(routing),
        )?,
    };

    let measured =
        measure_active_tokens(context, routing, section, ir, log, ownership.as_deref_mut()).await?;

    if measured < threshold {
        return Ok(ResponsesOuterOutcome::Ordinary {
            ir: ir.clone(),
            context_management: None,
            overlay_headers: replay_overlay,
        });
    }

    if let Some(checkpoint) = native_threshold_checkpoint(
        context,
        routing,
        ir,
        instructions,
        None,
        false,
        ledger,
        log,
        ownership.as_deref_mut(),
    )
    .await?
    {
        return Ok(ResponsesOuterOutcome::ContinueWithCheckpoint {
            prefix_items: vec![compaction_item(None, checkpoint.carrier, None)],
            artifact_id: checkpoint.artifact_id,
            ir: checkpoint.continuation,
            overlay_headers: checkpoint.overlay_headers,
            ledger: std::mem::take(ledger),
        });
    }

    // At or above the threshold: one compaction, then continuation. The
    // pending user turn's unit is mandatory retained state, and the answer
    // keeps its own output budget — the summary rides the configured budget
    // alone.
    let GatewaySummaryCheckpoint {
        plan,
        summary_text,
        carrier,
        artifact_id,
        ..
    } = run_gateway_summary(
        context,
        routing,
        ir,
        instructions,
        true,
        None,
        None,
        ReplacementMode::HistoricalCheckpoint,
        false,
        ledger,
        log,
        ownership,
    )
    .await?;

    Ok(ResponsesOuterOutcome::ContinueWithCheckpoint {
        prefix_items: vec![compaction_item(None, carrier, None)],
        artifact_id,
        ir: continuation_request(ir, &summary_text, &plan),
        overlay_headers: replay_overlay,
        ledger: std::mem::take(ledger),
    })
}

/// Measures the active input tokens of the restored window for one gateway
/// threshold evaluation: the definition's native counter when one is
/// configured and its dispatch succeeds, else the documented text estimator
/// (`docs/design/compaction.md`). The counter refines the measurement — its count is never
/// billed usage and records no ledger iteration — and its failure falls back
/// to the estimator, so it never gates the request on its own.
///
/// # Errors
///
/// Returns the estimator's [`CompactionError::UnsupportedMeasurement`] when no
/// counter is configured and the window carries content the estimator cannot
/// count, or a counter-dispatch failure's gateway error only when no fallback
/// exists — which cannot occur, the fallback always exists.
#[allow(clippy::too_many_arguments)]
async fn measure_active_tokens(
    context: &Context,
    routing: &ModelRouting,
    section: Option<&CompactionSection>,
    ir: &IrRequest,
    log: &mut ApiLogDraft,
    ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<u64, CompactionError> {
    let counter = section
        .and_then(|section| section.count_tokens.as_ref())
        .filter(|_operation| {
            routing.provider.definition.payload.format == UpstreamFormat::AnthropicMessages
        });

    if let Some(operation) = counter {
        if let Ok(measured) =
            native::count_tokens(context, routing, operation, ir, log, ownership).await
        {
            return Ok(measured);
        }

        // The counter dispatch failed: fall back to the estimator. The
        // dispatch already recorded its attempt; nothing else to undo.
        estimate_context_tokens(ir)
    } else {
        estimate_context_tokens(ir)
    }
}

/// The model's effective maximum input tokens as a non-negative capacity.
fn effective_input_capacity(routing: &ModelRouting) -> Option<u64> {
    routing
        .model
        .effective_max_input_tokens()
        .and_then(|capacity| u64::try_from(capacity).ok())
}

/// Executes a Messages completion request's compaction decision order and
/// returns the outer outcome the serving pipeline renders.
///
/// # Errors
///
/// Returns [`ApiError`] for every failure decision.
pub(crate) async fn execute_messages_request(
    context: &Context,
    routing: &ModelRouting,
    parsed: &ParsedRequest,
    preparation: &crate::compaction_model::boundary::BoundaryPreparation,
    log: &mut ApiLogDraft,
    mut ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<MessagesOuterOutcome, ApiError> {
    let format = routing.provider.definition.payload.format;

    let Some(compaction) = parsed.compaction.as_ref() else {
        return Ok(MessagesOuterOutcome::Ordinary {
            ir: parsed.ir.clone(),
            threshold_edits: None,
            overlay_headers: Vec::new(),
        });
    };

    let mut ir = parsed.ir.clone();

    let (restored, instructions) =
        history::prepare_for_target(&mut ir, compaction, routing, preparation)
            .map_err(api_error)?;

    let replay_overlay = replay_overlay_headers(
        routing.provider.definition.payload.compaction.as_ref(),
        format,
        restored.as_ref(),
    );

    let Some(control) = compaction.control.clone() else {
        return Ok(MessagesOuterOutcome::Ordinary {
            ir,
            threshold_edits: None,
            overlay_headers: replay_overlay,
        });
    };

    let mut ledger = CompactionLedger::default();

    match control {
        CompactionControl::MessagesOnDemand {
            instructions: compaction_instructions,
        } => messages_on_demand_flow(
            context,
            routing,
            &ir,
            &instructions,
            compaction_instructions,
            &mut ledger,
            log,
            ownership.as_deref_mut(),
        )
        .await
        .map_err(api_error),

        CompactionControl::MessagesThreshold { edits } => messages_threshold_flow(
            context,
            routing,
            &ir,
            &instructions,
            &edits,
            replay_overlay,
            &mut ledger,
            log,
            ownership,
        )
        .await
        .map_err(api_error),

        CompactionControl::ResponsesStandalone
        | CompactionControl::ResponsesTrigger
        | CompactionControl::ResponsesAutomatic { .. } => {
            Err(ApiError::from(GatewayError::Internal {
                message: "a Responses compaction control reached the Messages surface".to_string(),
            }))
        }
    }
}

/// Resolves the Messages on-demand decision order: the native on-demand
/// operation, then the standalone adapter wrapping its window in a Messages
/// carrier, then gateway summary. A positively classified unsupported boundary
/// at any native stage falls through to the next mechanism; every other
/// failure propagates unchanged.
#[expect(
    clippy::too_many_arguments,
    reason = "one same-account stage keeps its protocol controls, billing, and binding together"
)]
async fn messages_on_demand_flow(
    context: &Context,
    routing: &ModelRouting,
    ir: &IrRequest,
    instructions: &CheckpointInstructions<'_>,
    compaction_instructions: Option<String>,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
    mut ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<MessagesOuterOutcome, CompactionError> {
    let definition = &routing.provider.definition.payload;

    let section = definition.compaction.as_ref();

    let on_demand =
        section.and_then(|section| section.mechanisms.messages_on_demand_compaction.as_ref());

    // The exact native mechanism. The declaration's existence gates the stage:
    // the effective support resolves from model facts or an administrator
    // override even when the declared default carries no headers, and an
    // absent overlay dispatches with none.
    let on_demand_attempt = try_native(
        on_demand
            .filter(|_declaration| {
                is_native_eligible(
                    routing,
                    section,
                    Mechanism::MessagesOnDemandCompaction,
                    UpstreamFormat::AnthropicMessages,
                )
            })
            .map(|declaration| async {
                // Compact-only: the caller's output limit caps the on-demand budget.
                native::messages_on_demand(
                    context,
                    routing,
                    declaration.overlay_headers.as_deref().unwrap_or(&[]),
                    declaration.unsupported.as_ref(),
                    ir,
                    compaction_instructions.as_deref(),
                    summary_output_limit(section, ir.max_output_tokens),
                    ledger,
                    log,
                    ownership.as_deref_mut(),
                )
                .await
            }),
    )
    .await;

    match on_demand_attempt {
        NativeAttempt::Committed(native::OnDemandOutcome::Success { block, iterations }) => {
            return Ok(MessagesOuterOutcome::CompactionOnly {
                block: block.into(),
                artifact_id: None,
                iterations,
                ledger: std::mem::take(ledger),
            });
        }

        // The documented native no-summary outcome: original history stands;
        // the consumed response renders with its own semantics.
        NativeAttempt::Committed(native::OnDemandOutcome::NoSummary { aggregation }) => {
            return Ok(MessagesOuterOutcome::NativeNoSummary {
                aggregation,
                ledger: std::mem::take(ledger),
            });
        }

        NativeAttempt::Unsupported | NativeAttempt::NotEligible => {}

        NativeAttempt::Failed(error) => return Err(CompactionError::Upstream(error)),
    }

    // The cross-format adapter: the standalone operation compacts, and its
    // canonical window seals into a Messages carrier whose visible content is
    // the fixed transport description, bound into the carrier signature.
    let operation = section
        .and_then(|section| section.mechanisms.responses_standalone_compact.as_ref())
        .and_then(|declaration| declaration.operation.as_ref());

    let standalone = try_native(
        operation
            .filter(|_operation| {
                is_native_eligible(
                    routing,
                    section,
                    Mechanism::ResponsesStandaloneCompact,
                    UpstreamFormat::OpenAiResponse,
                )
            })
            .map(|operation| async {
                native::responses_standalone(
                    context,
                    routing,
                    operation,
                    ir,
                    ledger,
                    log,
                    ownership.as_deref_mut(),
                )
                .await
            }),
    )
    .await;

    if let NativeAttempt::Committed(native) = standalone {
        let (envelope, carrier) = seal_native_window(
            native_window_payload(
                routing,
                Mechanism::ResponsesStandaloneCompact,
                NativeReplayWindow::ResponsesWindow {
                    items: native.output,
                },
                history::produced_prefix_identity(ir),
            ),
            ReplacementMode::LeadingWindow,
            Some(GATEWAY_CARRIER_DESCRIPTION),
        )?;

        return Ok(MessagesOuterOutcome::CompactionOnly {
            block: gateway_carrier_block(None, &carrier),
            artifact_id: Some(envelope.artifact_id),
            iterations: compaction_iterations(ledger),
            ledger: std::mem::take(ledger),
        });
    }

    if let NativeAttempt::Failed(error) = standalone {
        return Err(CompactionError::Upstream(error));
    }

    // Gateway summary, the chain's final mechanism. Compact-only: a final user
    // turn may be summarized, and the caller's output limit caps the summary.
    let GatewaySummaryCheckpoint {
        summary_text,
        carrier,
        artifact_id,
        ..
    } = run_gateway_summary(
        context,
        routing,
        ir,
        instructions,
        false,
        compaction_instructions.as_deref(),
        ir.max_output_tokens,
        ReplacementMode::LeadingWindow,
        true,
        ledger,
        log,
        ownership,
    )
    .await?;

    Ok(MessagesOuterOutcome::CompactionOnly {
        block: gateway_carrier_block(Some(&summary_text), &carrier),
        artifact_id: Some(artifact_id),
        iterations: compaction_iterations(ledger),
        ledger: std::mem::take(ledger),
    })
}

/// Resolves the Messages threshold decision order: native threshold
/// passthrough, else the gateway-emulated threshold workflow with pause and
/// continuation semantics.
///
/// # Errors
///
/// Returns [`CompactionError`] for every failure decision.
#[expect(
    clippy::too_many_arguments,
    reason = "the flow's inputs are the passthrough edits, the replay overlay, and the \
              accounting facts the decision order resolves together"
)]
async fn messages_threshold_flow(
    context: &Context,
    routing: &ModelRouting,
    ir: &IrRequest,
    instructions: &CheckpointInstructions<'_>,
    edits: &[compaction_control::ThresholdEdit],
    replay_overlay: Vec<ParamRow>,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
    mut ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<MessagesOuterOutcome, CompactionError> {
    let definition = &routing.provider.definition.payload;

    let section = definition.compaction.as_ref();

    // Native threshold: the edits pass through upstream verbatim; the
    // block-preserving render rides the ordinary pipeline under the threshold
    // beta the declaration pins.
    if section.is_none_or(|section| section.strategy == CompactionStrategy::NativePreferred)
        && effective_support(routing, section, Mechanism::MessagesThresholdCompaction)
            == MechanismSupport::Supported
        && definition.format == UpstreamFormat::AnthropicMessages
    {
        let threshold_overlay = section
            .and_then(|section| section.mechanisms.messages_threshold_compaction.as_ref())
            .and_then(|declaration| declaration.overlay_headers.as_deref())
            .unwrap_or(&[])
            .to_vec();

        return Ok(MessagesOuterOutcome::Ordinary {
            ir: ir.clone(),
            threshold_edits: Some(native::threshold_edits(edits)),
            // The passthrough dispatch rides the threshold declaration's own
            // overlay, and a restored block replaying beside the control rides
            // its source mechanism's overlay on the same request — same-named
            // rows join, so the restored block never travels under the
            // threshold beta alone (`docs/design/compaction.md`).
            overlay_headers: combine_overlay_rows(threshold_overlay, replay_overlay),
        });
    }

    // Gateway emulation: the first edit's stated trigger drives the evaluation,
    // defaulting to the protocol's 150,000 tokens.
    let first_edit = edits.first();

    let threshold = first_edit
        .and_then(|edit| edit.trigger)
        .unwrap_or(MESSAGES_DEFAULT_THRESHOLD);

    let measured =
        measure_active_tokens(context, routing, section, ir, log, ownership.as_deref_mut()).await?;

    if measured < threshold {
        return Ok(MessagesOuterOutcome::Ordinary {
            ir: ir.clone(),
            threshold_edits: None,
            overlay_headers: replay_overlay,
        });
    }

    // At or above the threshold: one compaction. The pending turn's unit is
    // mandatory retained state; the summary rides the configured budget alone,
    // keeping the answer's own output budget separate.
    let edit_instructions = first_edit.and_then(|edit| edit.instructions.clone());

    if let Some(checkpoint) = native_threshold_checkpoint(
        context,
        routing,
        ir,
        instructions,
        edit_instructions.as_deref(),
        true,
        ledger,
        log,
        ownership.as_deref_mut(),
    )
    .await?
    {
        let block =
            gateway_carrier_block(checkpoint.visible_content.as_deref(), &checkpoint.carrier);
        let iterations = compaction_iterations(ledger);
        if first_edit.is_some_and(|edit| edit.pause_after_compaction) {
            return Ok(MessagesOuterOutcome::CompactionOnly {
                block,
                artifact_id: Some(checkpoint.artifact_id),
                iterations,
                ledger: std::mem::take(ledger),
            });
        }
        return Ok(MessagesOuterOutcome::ContinueWithCheckpoint {
            prefix_blocks: vec![block],
            artifact_id: checkpoint.artifact_id,
            ir: checkpoint.continuation,
            overlay_headers: checkpoint.overlay_headers,
            iterations,
            ledger: std::mem::take(ledger),
        });
    }

    let GatewaySummaryCheckpoint {
        plan,
        summary_text,
        carrier,
        artifact_id,
        ..
    } = run_gateway_summary(
        context,
        routing,
        ir,
        instructions,
        true,
        edit_instructions.as_deref(),
        None,
        ReplacementMode::HistoricalCheckpoint,
        true,
        ledger,
        log,
        ownership,
    )
    .await?;

    let block = gateway_carrier_block(Some(&summary_text), &carrier);

    let iterations = compaction_iterations(ledger);

    if first_edit.is_some_and(|edit| edit.pause_after_compaction) {
        return Ok(MessagesOuterOutcome::CompactionOnly {
            block,
            artifact_id: Some(artifact_id),
            iterations,
            ledger: std::mem::take(ledger),
        });
    }

    Ok(MessagesOuterOutcome::ContinueWithCheckpoint {
        prefix_blocks: vec![block],
        artifact_id,
        ir: continuation_request(ir, &summary_text, &plan),
        overlay_headers: replay_overlay,
        iterations,
        ledger: std::mem::take(ledger),
    })
}
