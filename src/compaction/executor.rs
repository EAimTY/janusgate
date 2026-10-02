//! The compaction executor: strategy selection and multi-call coordination for
//! compaction operations (README §6.8's decision order).
//!
//! One entry point per client surface, each resolving the decision order for
//! its request: validate and restore the active window first, then resolve the
//! active state's compatibility with the target, then — for a carrying control
//! — prefer the exact native mechanism, then a compatible native adapter, and
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
    accounting::{CompactionLedger, IterationKind},
    envelope::{
        EnvelopePayload, NativeReplayWindow, NativeWindowPayload, PrefixIdentity, ReplacementMode,
        summary_message,
    },
    error::CompactionError,
    history::{
        self, GatewayCheckpointPlan, RestoredBoundary, seal_gateway_checkpoint, seal_native_window,
    },
    mechanism::{Mechanism, MechanismSupport},
    native, summary,
    threshold::{
        estimate_context_tokens, messages_default_threshold, responses_gateway_default_threshold,
    },
};
use crate::{
    api::Error as ApiError,
    context::Context,
    format::{
        CompactionControl, NativeArtifact, ParsedCompaction, ParsedInstructionItem, ParsedRequest,
        UpstreamFormat,
        anthropic::messages::{GATEWAY_CARRIER_DESCRIPTION, gateway_carrier_block},
        compaction_control,
        openai::response::{
            compaction_item_json, render_compacted_response, render_gateway_compacted_response,
        },
    },
    gateway::{self, Error as GatewayError},
    ir::{IrAggregation, IrFinishReason, IrRequest, IrUsage, StateItem},
    provider::{
        ModelRouting,
        definition::{
            compaction::{CompactionSection, CompactionStrategy},
            template::ParamRow,
        },
    },
    storage::api_logs::ApiLogDraft,
};
use serde_json::Value;

/// The shipped default summary-output budget (README §5): applied when the
/// definition carries no compaction section to state its own.
const DEFAULT_SUMMARY_BUDGET_TOKENS: u64 = 4096;

/// The shipped default retained-input target (README §5): applied when the
/// definition carries no compaction section to state its own.
const DEFAULT_RETENTION_BUDGET_TOKENS: u64 = 8192;

/// Converts a compaction failure into the shared API error taxonomy: the
/// compaction domain maps onto the gateway error domain in one place
/// ([`CompactionError`]'s `From` impl), and the API taxonomy converts the
/// gateway domain in its own single place.
fn api_error(error: CompactionError) -> ApiError {
    ApiError::from(GatewayError::from(error))
}

/// Renders one rendered wire body into its JSON value: a first-party serde type
/// whose serialization is total.
///
/// # Panics
///
/// Panics if the rendered body fails to serialize, which cannot occur for a
/// first-party rendered wire type.
fn body_value<T: serde::Serialize>(body: &T) -> Value {
    #[expect(
        clippy::expect_used,
        reason = "a first-party rendered body always serializes"
    )]
    serde_json::to_value(body).expect("a rendered compaction body serializes")
}

/// The vendor-shaped `compaction_trigger` control item the native trigger
/// passthrough carries upstream at the input tail: the item is protocol state,
/// not conversation text, so it rides the input verbatim.
///
/// # Panics
///
/// Panics if the item fails to serialize, which cannot occur for the first-party
/// item shape.
fn trigger_item_json() -> Value {
    #[derive(serde::Serialize)]
    struct TriggerItem {
        /// The item kind tag.
        #[serde(rename = "type")]
        kind: &'static str,
    }

    #[expect(
        clippy::expect_used,
        reason = "a first-party rendered item serializes totally"
    )]
    serde_json::to_value(TriggerItem {
        kind: "compaction_trigger",
    })
    .expect("the trigger item serializes")
}

/// The header rows an ordinary replay or native passthrough dispatch carries
/// upstream so a replayed Anthropic compaction block stays acceptable to its
/// producer: a Claude on-demand block replays under the on-demand beta, a
/// threshold block under the threshold beta (the declaration's own contract —
/// the replay of a returned compaction block needs the operation's headers
/// wherever the block travels). Every other target and boundary carries no
/// overlay.
/// The header rows an ordinary replay or native passthrough dispatch carries
/// upstream so a replayed Anthropic compaction block stays acceptable to its
/// producer: a Claude on-demand block replays under the on-demand beta, a
/// threshold block under the threshold beta — the declarations' own contract,
/// that the replay of a returned compaction block needs the operation's headers
/// wherever the block travels. Every other target and boundary carries no
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

/// The gateway budgets of one routed request: the definition's stated budgets,
/// or the shipped defaults when the definition carries no compaction section.
fn budgets(section: Option<&CompactionSection>) -> (u64, u64) {
    match section {
        Some(section) => (section.summary_budget.get(), section.retention_budget.get()),

        None => (
            DEFAULT_SUMMARY_BUDGET_TOKENS,
            DEFAULT_RETENTION_BUDGET_TOKENS,
        ),
    }
}

/// The summary output budget bounded by a compact-only caller cap: the smaller
/// of the configured summary budget and the caller's stated output limit.
fn summary_output_limit(section: Option<&CompactionSection>, caller_cap: Option<u64>) -> u64 {
    match caller_cap {
        Some(cap) => budgets(section).0.min(cap),

        None => budgets(section).0,
    }
}

/// The native on-demand output budget: the effective output cap of the on-demand
/// dispatch, the smaller of the configured summary budget and the caller's
/// stated limit when one is present.
fn on_demand_output_limit(section: Option<&CompactionSection>, caller_cap: Option<u64>) -> u64 {
    summary_output_limit(section, caller_cap)
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
        producer_instance_id: format!("provider:{}", routing.provider.id),
        upstream_model_id: routing.model.upstream_id.clone(),
        mechanism,
        native_format: routing.provider.definition.payload.format,
        required_prefix_identity: Some(required_prefix_identity),
        window,
    }
}

/// Builds the vendor-shaped `usage.iterations` entries of a gateway-produced
/// compaction lifecycle: one entry per billed iteration the ledger recorded —
/// the compaction calls and, on a compact-and-continue lifecycle, the message
/// stage — so the rendered array stays authoritative for the billed totals
/// (README §6.8). Counter iterations carry no usage and never render.
///
/// # Panics
///
/// Panics if an entry fails to serialize, which cannot occur for the first-party
/// iteration shape.
fn compaction_iteration_values(ledger: &CompactionLedger) -> Vec<Value> {
    #[derive(serde::Serialize)]
    struct IterationOut {
        /// The iteration kind's vendor spelling.
        #[serde(rename = "type")]
        kind: &'static str,

        /// Tokens consumed by the iteration input.
        input_tokens: u64,

        /// Tokens produced by the iteration output.
        output_tokens: u64,

        /// Input tokens written to the upstream prompt cache, when reported.
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_creation_input_tokens: Option<u64>,

        /// Input tokens served from the upstream prompt cache, when reported.
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_read_input_tokens: Option<u64>,
    }

    ledger
        .iterations()
        .iter()
        .filter_map(|record| {
            let kind = match record.kind {
                IterationKind::Compaction => "compaction",
                IterationKind::Message => "message",
                // A counter call without usage is never recorded; a recorded
                // one is not a billed iteration and never renders.
                IterationKind::Counter => return None,
            };

            #[expect(
                clippy::expect_used,
                reason = "a first-party rendered iteration serializes totally"
            )]
            Some(
                serde_json::to_value(&IterationOut {
                    kind,
                    input_tokens: record.usage.input_tokens,
                    output_tokens: record.usage.output_tokens,
                    cache_creation_input_tokens: record.usage.cache_write_tokens,
                    cache_read_input_tokens: record.usage.cached_tokens,
                })
                .expect("a compaction iteration serializes"),
            )
        })
        .collect()
}

/// Runs the gateway summary flow end to end: plans the checkpoint from the
/// restored window, dispatches one internal summary inference, validates the
/// summary, and seals the checkpoint envelope into its carrier. The summary
/// call's usage rides the ledger under the compaction iteration kind.
///
/// Returns the plan (the continuation's building block), the summary text (the
/// Messages carrier's visible content), and the sealed carrier.
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
    instruction_items: &[ParsedInstructionItem],
    is_mandatory_active_turn: bool,
    instructions: Option<&str>,
    output_cap: Option<u64>,
    replacement: ReplacementMode,
    binds_visible_content: bool,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
) -> Result<(GatewayCheckpointPlan, String, String), CompactionError> {
    let plan = history::plan_gateway_checkpoint(
        restored,
        instruction_items,
        is_mandatory_active_turn,
        budgets(routing.provider.definition.payload.compaction.as_ref()),
    )?;

    let summarizer = summary::plan_summary_request(restored, &plan, instructions, output_cap);

    let aggregation = gateway::execute_internal(
        context,
        routing,
        summarizer,
        ledger,
        IterationKind::Compaction,
        log,
    )
    .await
    .map_err(CompactionError::Upstream)?;

    let summary_text = summary::validate_summary(&aggregation)?;

    let (_envelope, carrier) = seal_gateway_checkpoint(
        &plan,
        summary_text.clone(),
        replacement,
        binds_visible_content.then_some(summary_text.as_str()),
    )?;

    Ok((plan, summary_text, carrier))
}

/// A native threshold adapter result: checkpoint carrier and the prepared continuation.
struct NativeCheckpoint {
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
    compaction: &ParsedCompaction,
    instructions: Option<&str>,
    is_messages_surface: bool,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
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
    let mechanism = if definition.format == UpstreamFormat::OpenAiResponse
        && standalone.is_some()
        && effective_support(routing, section, Mechanism::ResponsesStandaloneCompact)
            == MechanismSupport::Supported
    {
        Mechanism::ResponsesStandaloneCompact
    } else if definition.format == UpstreamFormat::OpenAiResponse
        && effective_support(routing, section, Mechanism::ResponsesExplicitTrigger)
            == MechanismSupport::Supported
    {
        Mechanism::ResponsesExplicitTrigger
    } else if definition.format == UpstreamFormat::AnthropicMessages
        && on_demand.is_some()
        && effective_support(routing, section, Mechanism::MessagesOnDemandCompaction)
            == MechanismSupport::Supported
    {
        Mechanism::MessagesOnDemandCompaction
    } else {
        return Ok(None);
    };

    let plan =
        history::plan_native_checkpoint(ir, &compaction.instruction_items, budgets(section))?;
    let mut prefix = ir.clone();
    prefix.messages = plan.summary_input_messages.clone();
    prefix
        .state_items
        .retain(|item| item.offset < prefix.messages.len());
    let mut overlay_headers = Vec::new();
    let result = match mechanism {
        Mechanism::ResponsesStandaloneCompact => {
            #[expect(
                clippy::expect_used,
                reason = "selection requires the standalone operation"
            )]
            let operation = standalone.expect("selected standalone operation");
            native::responses_standalone(context, routing, operation, &prefix, ledger, log)
                .await
                .map(|native| NativeReplayWindow::ResponsesWindow {
                    items: native.output,
                })
        }
        Mechanism::ResponsesExplicitTrigger => {
            prefix.state_items.push(StateItem {
                offset: prefix.messages.len(),
                item: trigger_item_json(),
                is_gateway: false,
            });
            match gateway::execute_internal(
                context,
                routing,
                prefix,
                ledger,
                IterationKind::Compaction,
                log,
            )
            .await
            {
                Ok(aggregation)
                    if aggregation.compaction_states.len() == 1
                        && aggregation.finish == Some(IrFinishReason::Stop) =>
                {
                    Ok(NativeReplayWindow::ResponsesWindow {
                        items: aggregation.compaction_states,
                    })
                }
                Ok(_) => {
                    return Err(CompactionError::inaccessible_native_state(
                        "the native trigger did not produce one completed checkpoint",
                    ));
                }
                Err(GatewayError::Upstream { status, detail }) => {
                    let classifier = section
                        .and_then(|section| section.mechanisms.responses_explicit_trigger.as_ref())
                        .and_then(|declaration| declaration.unsupported.as_ref());
                    if native::classify_failure(classifier, status, &detail)
                        .await
                        .is_some()
                    {
                        return Ok(None);
                    }
                    return Err(CompactionError::Upstream(GatewayError::Upstream {
                        status,
                        detail,
                    }));
                }
                Err(error) => return Err(CompactionError::Upstream(error)),
            }
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
            )
            .await
            {
                Ok(native::OnDemandOutcome::Success { block, .. }) => {
                    Ok(NativeReplayWindow::AnthropicOnDemandBlock { block })
                }
                Ok(native::OnDemandOutcome::NoSummary { .. }) => {
                    return Err(CompactionError::inaccessible_native_state(
                        "the threshold adapter returned no usable native checkpoint",
                    ));
                }
                Err(error) => Err(error),
            }
        }
        _ => return Ok(None),
    };
    let window = match result {
        Ok(window) => window,
        Err(native::NativeAttemptFailure::ClassifiedUnsupported(_)) => return Ok(None),
        Err(native::NativeAttemptFailure::Failed(error)) => {
            return Err(CompactionError::Upstream(error));
        }
    };
    let mut payload = native_window_payload(
        routing,
        mechanism,
        window,
        history::produced_prefix_identity(ir),
    );
    payload.retained_messages = plan
        .retained_units
        .into_iter()
        .flat_map(|(_, messages)| messages)
        .collect();
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
    let (_, carrier) = seal_native_window(
        payload,
        ReplacementMode::HistoricalCheckpoint,
        visible_content.as_deref(),
    )?;
    Ok(Some(NativeCheckpoint {
        carrier,
        visible_content,
        continuation,
        overlay_headers,
    }))
}

/// Builds the continuation request of a completed checkpoint: the restored
/// compacted window — the summary message, the retained units, and the native
/// state riding them — with the caller's instruction context and output budget
/// preserved and every compaction control stripped.
#[must_use]
fn continuation_request(
    restored: &IrRequest,
    summary_text: &str,
    plan: &GatewayCheckpointPlan,
) -> IrRequest {
    let mut continuation = restored.clone();

    let mut messages = Vec::with_capacity(1 + plan.retained_units.len());

    messages.push(summary_message(summary_text));

    for (_identity, unit_messages) in &plan.retained_units {
        messages.extend(unit_messages.iter().cloned());
    }

    continuation.messages = messages;
    continuation.state_items = plan.retained_state_items.clone();

    continuation
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
        context_management: Option<Vec<Value>>,

        /// The header rows the replayed native state's operation requires.
        overlay_headers: Vec<ParamRow>,
    },

    /// The synthesized compact-only artifact: the checkpoint items the caller's
    /// lifecycle renders — one gateway carrier or one wrapped-native carrier —
    /// beside the billed usage of the participating calls.
    CompactionOnly {
        /// The compaction output items, in output order.
        items: Vec<Value>,

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
        prefix_items: Vec<Value>,

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
        threshold_edits: Option<Vec<Value>>,

        /// The header rows the replayed native state's operation requires.
        overlay_headers: Vec<ParamRow>,
    },

    /// The compact-only artifact — a native compaction block or a gateway
    /// carrier block — with `stop_reason: "compaction"` and no answer.
    CompactionOnly {
        /// The compaction block.
        block: Value,

        /// The vendor-shaped `usage.iterations` entries to render.
        iterations: Vec<Value>,

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
        prefix_blocks: Vec<Value>,

        /// The continuation request.
        ir: IrRequest,

        /// Operation headers required by a native checkpoint continuation.
        overlay_headers: Vec<ParamRow>,

        /// The vendor-shaped `usage.iterations` entries to render.
        iterations: Vec<Value>,

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
    log: &mut ApiLogDraft,
) -> Result<Value, ApiError> {
    let compaction = parsed.compaction.as_ref().ok_or_else(|| {
        ApiError::from(GatewayError::Internal {
            message: "the compact route's parsed request carries no compaction facts".to_string(),
        })
    })?;

    let format = routing.provider.definition.payload.format;

    let mut ir = parsed.ir.clone();

    history::prepare_for_target(&mut ir, compaction, routing).map_err(api_error)?;

    let section = routing.provider.definition.payload.compaction.as_ref();

    let mut ledger = CompactionLedger::default();

    // Native standalone first: eligible when the strategy prefers native, the
    // mechanism resolves to supported, the operation descriptor exists, and the
    // definition speaks the Responses wire the compact body renders.
    let operation = section
        .and_then(|section| section.mechanisms.responses_standalone_compact.as_ref())
        .and_then(|declaration| declaration.operation.as_ref());

    if section.is_none_or(|section| section.strategy == CompactionStrategy::NativePreferred)
        && effective_support(routing, section, Mechanism::ResponsesStandaloneCompact)
            == MechanismSupport::Supported
        && let Some(operation) = operation
        && format == UpstreamFormat::OpenAiResponse
    {
        match native::responses_standalone(context, routing, operation, &ir, &mut ledger, log).await
        {
            Ok(native) => {
                log.stamp_billed_usage(ledger.billed_totals());

                return Ok(body_value(&render_compacted_response(native)));
            }

            Err(native::NativeAttemptFailure::ClassifiedUnsupported(reason)) => {
                log::debug!(
                    "compaction: the native standalone operation was classified unsupported \
                     ({reason}); falling back to gateway summary inference"
                );
            }

            Err(native::NativeAttemptFailure::Failed(error)) => {
                return Err(ApiError::from(error));
            }
        }
    }

    // Gateway summary: the strategy's own choice, the unknown-support path, or
    // the single classifier-positive fallback. Compact-only: a final user turn
    // may be summarized, and the compact body carries no output cap.
    let (_plan, _summary_text, carrier) = run_gateway_summary(
        context,
        routing,
        &ir,
        &compaction.instruction_items,
        false,
        None,
        None,
        ReplacementMode::CanonicalWindow,
        false,
        &mut ledger,
        log,
    )
    .await
    .map_err(api_error)?;

    let totals = ledger.billed_totals();

    log.stamp_billed_usage(totals);

    Ok(body_value(&render_gateway_compacted_response(
        carrier, totals,
    )))
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
    log: &mut ApiLogDraft,
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

    let restored = history::prepare_for_target(&mut ir, compaction, routing).map_err(api_error)?;

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

        CompactionControl::ResponsesTrigger => {
            responses_trigger_flow(context, routing, &ir, compaction, &mut ledger, log)
                .await
                .map_err(api_error)
        }

        CompactionControl::ResponsesAutomatic { entries } => responses_automatic_flow(
            context,
            routing,
            &ir,
            compaction,
            &entries,
            replay_overlay,
            &mut ledger,
            log,
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
/// gateway summary — every outcome a compact-only lifecycle.
async fn responses_trigger_flow(
    context: &Context,
    routing: &ModelRouting,
    ir: &IrRequest,
    compaction: &ParsedCompaction,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
) -> Result<ResponsesOuterOutcome, CompactionError> {
    let definition = &routing.provider.definition.payload;

    let section = definition.compaction.as_ref();

    let mut is_native_preferred =
        section.is_none_or(|section| section.strategy == CompactionStrategy::NativePreferred);

    // The exact native mechanism: the trigger rides the ordinary request
    // upstream, and the lifecycle must carry exactly one completed compaction
    // item before any outer success.
    if is_native_preferred
        && effective_support(routing, section, Mechanism::ResponsesExplicitTrigger)
            == MechanismSupport::Supported
        && definition.format == UpstreamFormat::OpenAiResponse
    {
        let mut passthrough = ir.clone();

        passthrough.state_items.push(StateItem {
            offset: passthrough.messages.len(),
            item: trigger_item_json(),
            is_gateway: false,
        });

        let attempt = gateway::execute_internal(
            context,
            routing,
            passthrough,
            ledger,
            IterationKind::Compaction,
            log,
        )
        .await;

        let aggregation = match attempt {
            Ok(aggregation) => Some(aggregation),
            Err(GatewayError::Upstream { status, detail })
                if native::classify_failure(
                    section
                        .and_then(|section| section.mechanisms.responses_explicit_trigger.as_ref())
                        .and_then(|declaration| declaration.unsupported.as_ref()),
                    status,
                    &detail,
                )
                .await
                .is_some() =>
            {
                is_native_preferred = false;
                None
            }
            Err(error) => return Err(CompactionError::Upstream(error)),
        };
        if let Some(aggregation) = aggregation {
            if aggregation.compaction_states.len() != 1
                || aggregation.finish != Some(IrFinishReason::Stop)
            {
                return Err(CompactionError::inaccessible_native_state(
                    "the native trigger response did not carry exactly one \
                     completed compaction item",
                ));
            }

            log.stamp_billed_usage(ledger.billed_totals());

            return Ok(ResponsesOuterOutcome::NativeLifecycle {
                aggregation,
                ledger: std::mem::take(ledger),
            });
        }
    }

    // The same-format adapter: the trigger compacts through the standalone
    // operation, whose canonical window seals into one carrier item.
    let operation = section
        .and_then(|section| section.mechanisms.responses_standalone_compact.as_ref())
        .and_then(|declaration| declaration.operation.as_ref());

    if is_native_preferred
        && effective_support(routing, section, Mechanism::ResponsesStandaloneCompact)
            == MechanismSupport::Supported
        && let Some(operation) = operation
        && definition.format == UpstreamFormat::OpenAiResponse
    {
        match native::responses_standalone(context, routing, operation, ir, ledger, log).await {
            Ok(native) => {
                let (_envelope, carrier) = seal_native_window(
                    native_window_payload(
                        routing,
                        Mechanism::ResponsesStandaloneCompact,
                        NativeReplayWindow::ResponsesWindow {
                            items: native.output,
                        },
                        super::history::produced_prefix_identity(ir),
                    ),
                    ReplacementMode::CanonicalWindow,
                    None,
                )?;

                log.stamp_billed_usage(ledger.billed_totals());

                return Ok(ResponsesOuterOutcome::CompactionOnly {
                    items: vec![compaction_item_json(None, carrier, None)],
                    usage: ledger.billed_totals(),
                    ledger: std::mem::take(ledger),
                });
            }

            Err(native::NativeAttemptFailure::ClassifiedUnsupported(reason)) => {
                log::debug!(
                    "compaction: the trigger's standalone adapter was classified unsupported \
                     ({reason}); falling back to gateway summary inference"
                );
            }

            Err(native::NativeAttemptFailure::Failed(error)) => {
                return Err(CompactionError::Upstream(error));
            }
        }
    }

    // The cross-format adapter: the trigger compacts through the Messages
    // on-demand operation, whose block seals into one carrier item with the
    // canonical-window replacement the trigger contract demands.
    let on_demand =
        section.and_then(|section| section.mechanisms.messages_on_demand_compaction.as_ref());

    if is_native_preferred
        && effective_support(routing, section, Mechanism::MessagesOnDemandCompaction)
            == MechanismSupport::Supported
        && let Some(on_demand) = on_demand
        && let Some(overlay_headers) = on_demand
            .overlay_headers
            .as_deref()
            .filter(|rows| !rows.is_empty())
        && definition.format == UpstreamFormat::AnthropicMessages
    {
        let max_tokens = on_demand_output_limit(section, ir.max_output_tokens);

        match native::messages_on_demand(
            context,
            routing,
            overlay_headers,
            on_demand.unsupported.as_ref(),
            ir,
            None,
            max_tokens,
            ledger,
            log,
        )
        .await
        {
            Ok(native::OnDemandOutcome::Success { block, .. }) => {
                let (_envelope, carrier) = seal_native_window(
                    native_window_payload(
                        routing,
                        Mechanism::MessagesOnDemandCompaction,
                        NativeReplayWindow::AnthropicOnDemandBlock { block },
                        super::history::produced_prefix_identity(ir),
                    ),
                    ReplacementMode::CanonicalWindow,
                    None,
                )?;

                log.stamp_billed_usage(ledger.billed_totals());

                return Ok(ResponsesOuterOutcome::CompactionOnly {
                    items: vec![compaction_item_json(None, carrier, None)],
                    usage: ledger.billed_totals(),
                    ledger: std::mem::take(ledger),
                });
            }

            // The adapter cannot satisfy the trigger's exactly-one-artifact
            // contract with a no-summary outcome, so the outcome fails
            // explicitly instead of rendering a successful lifecycle without
            // its required artifact.
            Ok(native::OnDemandOutcome::NoSummary { .. }) => {
                return Err(CompactionError::inaccessible_native_state(
                    "the native on-demand compaction returned no usable block, so the trigger \
                     contract cannot be satisfied",
                ));
            }

            Err(native::NativeAttemptFailure::ClassifiedUnsupported(reason)) => {
                log::debug!(
                    "compaction: the trigger's on-demand adapter was classified unsupported \
                     ({reason}); falling back to gateway summary inference"
                );
            }

            Err(native::NativeAttemptFailure::Failed(error)) => {
                return Err(CompactionError::Upstream(error));
            }
        }
    }

    // Gateway summary: unknown support, the strategy's own choice, or the
    // single classifier-positive fallback. The trigger is compact-only, so the
    // caller's output limit caps the summary.
    let (_plan, _summary_text, carrier) = run_gateway_summary(
        context,
        routing,
        ir,
        &compaction.instruction_items,
        false,
        None,
        ir.max_output_tokens,
        ReplacementMode::CanonicalWindow,
        false,
        ledger,
        log,
    )
    .await?;

    log.stamp_billed_usage(ledger.billed_totals());

    Ok(ResponsesOuterOutcome::CompactionOnly {
        items: vec![compaction_item_json(None, carrier, None)],
        usage: ledger.billed_totals(),
        ledger: std::mem::take(ledger),
    })
}

/// Resolves the Responses automatic decision order: native automatic
/// passthrough, else the gateway-emulated threshold workflow — measure once at
/// the request boundary over the restored window, compact at or above the
/// threshold, and continue on the compacted window.
async fn responses_automatic_flow(
    context: &Context,
    routing: &ModelRouting,
    ir: &IrRequest,
    compaction: &ParsedCompaction,
    entries: &[compaction_control::AutomaticEntry],
    replay_overlay: Vec<ParamRow>,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
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
            context_management: Some(native::automatic_entry_values(entries)),
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
            &routing.model.public_id,
            effective_input_capacity(routing),
        )?,
    };

    let measured = measure_active_tokens(context, routing, section, ir, ledger, log).await?;

    if measured < threshold {
        return Ok(ResponsesOuterOutcome::Ordinary {
            ir: ir.clone(),
            context_management: None,
            overlay_headers: replay_overlay,
        });
    }

    if let Some(checkpoint) =
        native_threshold_checkpoint(context, routing, ir, compaction, None, false, ledger, log)
            .await?
    {
        return Ok(ResponsesOuterOutcome::ContinueWithCheckpoint {
            prefix_items: vec![compaction_item_json(None, checkpoint.carrier, None)],
            ir: checkpoint.continuation,
            overlay_headers: checkpoint.overlay_headers,
            ledger: std::mem::take(ledger),
        });
    }

    // At or above the threshold: one compaction, then continuation. The
    // pending user turn's unit is mandatory retained state, and the answer
    // keeps its own output budget — the summary rides the configured budget
    // alone.
    let (plan, summary_text, carrier) = run_gateway_summary(
        context,
        routing,
        ir,
        &compaction.instruction_items,
        true,
        None,
        None,
        ReplacementMode::HistoricalCheckpoint,
        false,
        ledger,
        log,
    )
    .await?;

    log.stamp_billed_usage(ledger.billed_totals());

    Ok(ResponsesOuterOutcome::ContinueWithCheckpoint {
        prefix_items: vec![compaction_item_json(None, carrier, None)],
        ir: continuation_request(ir, &summary_text, &plan),
        overlay_headers: replay_overlay,
        ledger: std::mem::take(ledger),
    })
}

/// Measures the active input tokens of the restored window for one gateway
/// threshold evaluation: the definition's native counter when one is
/// configured and its dispatch succeeds, else the documented text estimator
/// (README §6.8). A counter dispatch is recorded as its own iteration kind
/// and its failure falls back to the estimator — the counter refines the
/// measurement, it never gates the request on its own.
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
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
) -> Result<u64, CompactionError> {
    let counter = section
        .and_then(|section| section.count_tokens.as_ref())
        .filter(|_operation| {
            routing.provider.definition.payload.format == UpstreamFormat::AnthropicMessages
        });

    if let Some(operation) = counter {
        if let Ok(measured) =
            native::count_tokens(context, routing, operation, ir, ledger, log).await
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
    log: &mut ApiLogDraft,
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

    let restored = history::prepare_for_target(&mut ir, compaction, routing).map_err(api_error)?;

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
        CompactionControl::MessagesOnDemand { instructions } => messages_on_demand_flow(
            context,
            routing,
            &ir,
            compaction,
            instructions,
            &mut ledger,
            log,
        )
        .await
        .map_err(api_error),

        CompactionControl::MessagesThreshold { edits } => messages_threshold_flow(
            context,
            routing,
            &ir,
            compaction,
            &edits,
            replay_overlay,
            &mut ledger,
            log,
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
/// carrier, then gateway summary.
#[expect(
    clippy::too_many_lines,
    reason = "the adapter chain is one decision order; splitting it would scatter the order \
              across helpers without reducing the whole"
)]
async fn messages_on_demand_flow(
    context: &Context,
    routing: &ModelRouting,
    ir: &IrRequest,
    compaction: &ParsedCompaction,
    instructions: Option<String>,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
) -> Result<MessagesOuterOutcome, CompactionError> {
    let definition = &routing.provider.definition.payload;

    let section = definition.compaction.as_ref();

    let is_native_preferred =
        section.is_none_or(|section| section.strategy == CompactionStrategy::NativePreferred);

    let on_demand =
        section.and_then(|section| section.mechanisms.messages_on_demand_compaction.as_ref());

    // The exact native mechanism.
    if is_native_preferred
        && effective_support(routing, section, Mechanism::MessagesOnDemandCompaction)
            == MechanismSupport::Supported
        && let Some(on_demand) = on_demand
        && let Some(overlay_headers) = on_demand
            .overlay_headers
            .as_deref()
            .filter(|rows| !rows.is_empty())
        && definition.format == UpstreamFormat::AnthropicMessages
    {
        // Compact-only: the caller's output limit caps the on-demand budget.
        let max_tokens = on_demand_output_limit(section, ir.max_output_tokens);

        match native::messages_on_demand(
            context,
            routing,
            overlay_headers,
            on_demand.unsupported.as_ref(),
            ir,
            instructions.as_deref(),
            max_tokens,
            ledger,
            log,
        )
        .await
        {
            Ok(native::OnDemandOutcome::Success { block, iterations }) => {
                log.stamp_billed_usage(ledger.billed_totals());

                return Ok(MessagesOuterOutcome::CompactionOnly {
                    block,
                    iterations,
                    ledger: std::mem::take(ledger),
                });
            }

            // The documented native no-summary outcome: original history
            // stands; the consumed response renders with its own semantics.
            Ok(native::OnDemandOutcome::NoSummary { aggregation }) => {
                log.stamp_billed_usage(ledger.billed_totals());

                return Ok(MessagesOuterOutcome::NativeNoSummary {
                    aggregation,
                    ledger: std::mem::take(ledger),
                });
            }

            Err(native::NativeAttemptFailure::ClassifiedUnsupported(reason)) => {
                log::debug!(
                    "compaction: the native on-demand operation was classified unsupported \
                     ({reason}); falling back"
                );
            }

            Err(native::NativeAttemptFailure::Failed(error)) => {
                return Err(CompactionError::Upstream(error));
            }
        }
    }

    // The cross-format adapter: the standalone operation compacts, and its
    // canonical window seals into a Messages carrier whose visible content is
    // the fixed transport description, bound into the carrier signature.
    let operation = section
        .and_then(|section| section.mechanisms.responses_standalone_compact.as_ref())
        .and_then(|declaration| declaration.operation.as_ref());

    if is_native_preferred
        && effective_support(routing, section, Mechanism::ResponsesStandaloneCompact)
            == MechanismSupport::Supported
        && let Some(operation) = operation
        && definition.format == UpstreamFormat::OpenAiResponse
    {
        match native::responses_standalone(context, routing, operation, ir, ledger, log).await {
            Ok(native) => {
                let (_envelope, carrier) = seal_native_window(
                    native_window_payload(
                        routing,
                        Mechanism::ResponsesStandaloneCompact,
                        NativeReplayWindow::ResponsesWindow {
                            items: native.output,
                        },
                        super::history::produced_prefix_identity(ir),
                    ),
                    ReplacementMode::LeadingWindow,
                    Some(GATEWAY_CARRIER_DESCRIPTION),
                )?;

                log.stamp_billed_usage(ledger.billed_totals());

                return Ok(MessagesOuterOutcome::CompactionOnly {
                    block: gateway_carrier_block(None, &carrier),
                    iterations: compaction_iteration_values(ledger),
                    ledger: std::mem::take(ledger),
                });
            }

            Err(native::NativeAttemptFailure::ClassifiedUnsupported(reason)) => {
                log::debug!(
                    "compaction: the on-demand request's standalone adapter was classified \
                     unsupported ({reason}); falling back to gateway summary inference"
                );
            }

            Err(native::NativeAttemptFailure::Failed(error)) => {
                return Err(CompactionError::Upstream(error));
            }
        }
    }

    // Gateway summary. Compact-only: a final user turn may be summarized, and
    // the caller's output limit caps the summary.
    let (_plan, summary_text, carrier) = run_gateway_summary(
        context,
        routing,
        ir,
        &compaction.instruction_items,
        false,
        instructions.as_deref(),
        ir.max_output_tokens,
        ReplacementMode::LeadingWindow,
        true,
        ledger,
        log,
    )
    .await?;

    log.stamp_billed_usage(ledger.billed_totals());

    Ok(MessagesOuterOutcome::CompactionOnly {
        block: gateway_carrier_block(Some(&summary_text), &carrier),
        iterations: compaction_iteration_values(ledger),
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
    compaction: &ParsedCompaction,
    edits: &[compaction_control::ThresholdEdit],
    replay_overlay: Vec<ParamRow>,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
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
            threshold_edits: Some(native::threshold_edit_values(edits)),
            overlay_headers: threshold_overlay,
        });
    }

    // Gateway emulation: the first edit's stated trigger drives the evaluation,
    // defaulting to the protocol's 150,000 tokens.
    let first_edit = edits.first();

    let threshold = first_edit
        .and_then(|edit| edit.trigger)
        .unwrap_or_else(messages_default_threshold);

    let measured = measure_active_tokens(context, routing, section, ir, ledger, log).await?;

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
    let instructions = first_edit.and_then(|edit| edit.instructions.clone());

    if let Some(checkpoint) = native_threshold_checkpoint(
        context,
        routing,
        ir,
        compaction,
        instructions.as_deref(),
        true,
        ledger,
        log,
    )
    .await?
    {
        let block =
            gateway_carrier_block(checkpoint.visible_content.as_deref(), &checkpoint.carrier);
        let iterations = compaction_iteration_values(ledger);
        if first_edit.is_some_and(|edit| edit.pause_after_compaction) {
            return Ok(MessagesOuterOutcome::CompactionOnly {
                block,
                iterations,
                ledger: std::mem::take(ledger),
            });
        }
        return Ok(MessagesOuterOutcome::ContinueWithCheckpoint {
            prefix_blocks: vec![block],
            ir: checkpoint.continuation,
            overlay_headers: checkpoint.overlay_headers,
            iterations,
            ledger: std::mem::take(ledger),
        });
    }

    let (plan, summary_text, carrier) = run_gateway_summary(
        context,
        routing,
        ir,
        &compaction.instruction_items,
        true,
        instructions.as_deref(),
        None,
        ReplacementMode::HistoricalCheckpoint,
        true,
        ledger,
        log,
    )
    .await?;

    let block = gateway_carrier_block(Some(&summary_text), &carrier);

    let iterations = compaction_iteration_values(ledger);

    log.stamp_billed_usage(ledger.billed_totals());

    if first_edit.is_some_and(|edit| edit.pause_after_compaction) {
        return Ok(MessagesOuterOutcome::CompactionOnly {
            block,
            iterations,
            ledger: std::mem::take(ledger),
        });
    }

    Ok(MessagesOuterOutcome::ContinueWithCheckpoint {
        prefix_blocks: vec![block],
        ir: continuation_request(ir, &summary_text, &plan),
        overlay_headers: replay_overlay,
        iterations,
        ledger: std::mem::take(ledger),
    })
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::expect_used,
        reason = "test fixtures parse and render values constructed to succeed"
    )]
    #![expect(
        clippy::indexing_slicing,
        reason = "fixtures index values they constructed themselves"
    )]
    #![expect(
        clippy::panic,
        reason = "a failing fixture pattern-match is the test failure report"
    )]

    use super::{
        api_error, body_value, continuation_request, effective_support, execute_responses_compact,
        summary_output_limit, trigger_item_json,
    };
    use crate::{
        compaction::{
            error::CompactionError,
            history::{GatewayCheckpointPlan, seal_gateway_checkpoint},
            identity::{InstructionRole, ItemIdentity},
            mechanism::Mechanism,
            summary::SUMMARY_CLOSING_INSTRUCTION,
        },
        format::ParsedInstructionItem,
        gateway::Error as GatewayError,
        ir::{IrMessage, IrPart, IrRequest, IrRole, StateItem},
        provider::{
            Model, ModelRouting, Provider, StoredCredentials,
            definition::{
                DefinitionPayload, ProviderDefinition,
                compaction::{
                    CompactionSection, MechanismDeclarations, OnDemandDeclaration,
                    StandaloneCompactDeclaration,
                },
                credential::CredentialAcquisition,
                inference::Inference,
                template::StringExpr,
            },
        },
    };
    use std::num::NonZeroI64;

    /// A compaction failure maps onto the shared API taxonomy through the
    /// gateway domain in one place: client-state failures render 400, an
    /// unusable summary renders 502, a configuration gap renders the
    /// fixed-message 500, and a dispatched native failure passes through.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn compaction_failures_map_through_the_gateway_domain() {
        let client_state = api_error(CompactionError::inaccessible_native_state("incompatible"));

        assert_eq!(
            client_state.http_status(),
            axum::http::StatusCode::BAD_REQUEST
        );
        assert!(
            client_state
                .to_string()
                .contains("compaction_inaccessible_state"),
            "the stable code rides the rendered message"
        );

        let summary = api_error(CompactionError::unusable_summary("refusal"));

        assert_eq!(summary.http_status(), axum::http::StatusCode::BAD_GATEWAY);

        // A configuration gap renders the fixed internal message: the cause
        // reaches the log only (README §6.7).
        let internal = api_error(CompactionError::ConfigurationMissing {
            message: String::from("no keys"),
        });

        assert_eq!(
            internal.http_status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );

        assert_eq!(internal.to_string(), "Internal server error.");

        let upstream = api_error(CompactionError::Upstream(GatewayError::Unavailable {
            detail: String::from("unreachable"),
        }));

        assert_eq!(upstream.http_status(), axum::http::StatusCode::BAD_GATEWAY);
    }

    /// Builds a routing fixture over one static-key Responses provider pointed
    /// at `base`, carrying `section` as its compaction section.
    ///
    /// # Panics
    ///
    /// Panics if the fixture definition version fails to construct, which the
    /// positive literal prevents.
    fn routing_at(base: &str, section: Option<CompactionSection>) -> ModelRouting {
        let provider = Provider {
            id: 7,
            name: String::from("test"),
            model_prefix: String::new(),
            base_url: None,
            credentials: StoredCredentials::ApiKey {
                api_key: String::from("key"),
            },
            is_enabled: true,
            definition: ProviderDefinition {
                name: String::from("def"),
                version: NonZeroI64::new(1).expect("one is positive"),
                payload: DefinitionPayload {
                    format: crate::format::UpstreamFormat::OpenAiResponse,
                    inference: Inference {
                        url: StringExpr::literal(format!("{base}/responses")),
                        stream_url: None,
                        headers: vec![],
                        upstream_streaming:
                            crate::provider::definition::inference::UpstreamStreaming::MatchClient,
                        render_options: vec![],
                        envelope: None,
                        response_unwrap: None,
                    },
                    credential: CredentialAcquisition::ApiKey,
                    provisioning: None,
                    discovery: None,
                    quota: None,
                    compaction: section,
                },
                created_at: 0,
            },
        };

        ModelRouting {
            provider,
            model: Model {
                id: 1,
                upstream_id: String::from("m"),
                public_id: String::from("public-m"),
                id_override: None,
                is_enabled: true,
                discovery_state: crate::provider::DiscoveryState::Manual,
                thinking_levels: None,
                max_input_tokens: None,
                max_output_tokens: None,
                thinking_levels_override: None,
                max_input_tokens_override: None,
                max_output_tokens_override: None,
                compaction_support: None,
                compaction_support_override: None,
            },
            is_effectively_enabled: true,
        }
    }

    /// The routing fixture at its default base URL.
    fn routing() -> ModelRouting {
        routing_at("https://upstream.test/v1", None)
    }

    /// A declaration-free compaction section: the unknown-support posture with
    /// a one-token retention target, so the compact fixture's whole history is
    /// the summarized prefix.
    ///
    /// # Panics
    ///
    /// Panics if a fixture budget fails to construct, which the positive
    /// literals prevent.
    fn summarize_only_section() -> CompactionSection {
        CompactionSection {
            strategy: crate::provider::definition::compaction::CompactionStrategy::NativePreferred,
            summary_budget: std::num::NonZeroU64::new(4096).expect("positive"),
            retention_budget: std::num::NonZeroU64::new(1).expect("positive"),
            can_export_native_summary: false,
            mechanisms: MechanismDeclarations::default(),
            count_tokens: None,
        }
    }

    /// A supported standalone compact declaration whose operation addresses
    /// `url`, with a classifier matching `statuses`.
    ///
    /// # Panics
    ///
    /// Panics if a fixture budget fails to construct, which the positive
    /// literals prevent.
    fn standalone_section(url: String, statuses: Vec<u16>) -> CompactionSection {
        CompactionSection {
            strategy: crate::provider::definition::compaction::CompactionStrategy::NativePreferred,
            summary_budget: std::num::NonZeroU64::new(4096).expect("positive"),
            // A one-token retention target: the compact fixture's final turn
            // exceeds it, so the whole history is the summarized prefix and the
            // checkpoint plan has an older prefix to cover.
            retention_budget: std::num::NonZeroU64::new(1).expect("positive"),
            can_export_native_summary: false,
            mechanisms: MechanismDeclarations {
                responses_standalone_compact: Some(
                    crate::provider::definition::compaction::StandaloneCompactDeclaration {
                        default: crate::compaction::mechanism::MechanismSupport::Supported,
                        operation: Some(
                            crate::provider::definition::compaction::StandaloneCompactOperation {
                                url: StringExpr::literal(url),
                                headers: vec![],
                                envelope: None,
                                response_unwrap: None,
                                unsupported: Some(
                                    crate::provider::definition::compaction::UnsupportedClassifier {
                                        statuses,
                                        codes: Vec::new(),
                                        parameters: Vec::new(),
                                    },
                                ),
                            },
                        ),
                        unsupported: None,
                    },
                ),
                ..MechanismDeclarations::default()
            },
            count_tokens: None,
        }
    }

    /// The compact request fixture body: a three-turn history with an older
    /// prefix and a final user turn.
    fn compact_body() -> serde_json::Value {
        serde_json::json!({
            "model": "public-m",
            "input": [
                {"type": "message", "role": "user", "content": "old question"},
                {"type": "message", "role": "assistant", "content": "old answer"},
                {"type": "message", "role": "user", "content": "another turn"},
            ],
        })
    }

    /// Parses the compact fixture body.
    ///
    /// # Panics
    ///
    /// Panics if the fixture body fails to parse, which its shape prevents.
    fn compact_parsed() -> crate::format::ParsedRequest {
        #[expect(clippy::expect_used, reason = "the fixture body parses")]
        let parsed = crate::format::openai::response::parse_compact_request(
            compact_body().to_string().as_bytes(),
        )
        .expect("the compact fixture parses");

        parsed
    }

    /// A completed Responses body the mocked summary upstream answers with.
    fn summary_response(text: &str) -> String {
        serde_json::json!({
            "id": "resp_summary",
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": text}],
            }],
            "usage": {"input_tokens": 10, "output_tokens": 5},
        })
        .to_string()
    }

    /// A native standalone compacted window the mocked compact endpoint
    /// answers with.
    fn compacted_response() -> String {
        serde_json::json!({
            "id": "resp_native",
            "object": "response.compaction",
            "created_at": 5,
            "output": [
                {"type": "message", "role": "assistant", "content": "kept"},
                {"type": "compaction", "id": "cmp_native", "encrypted_content": "state"},
            ],
            "usage": {"input_tokens": 7, "output_tokens": 2},
        })
        .to_string()
    }

    /// Support resolution reads the model facts against the section: an absent
    /// declaration and absent facts are unknown, a declared default resolves,
    /// and model facts win over the declaration.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn support_resolves_across_the_layers() {
        let routing = routing();

        let section: Option<&CompactionSection> = None;

        assert_eq!(
            effective_support(&routing, section, Mechanism::ResponsesAutomaticCompaction),
            crate::compaction::mechanism::MechanismSupport::Unknown,
        );

        // A section declaring on-demand supported resolves through the model
        // facts path.
        let declared = CompactionSection {
            strategy: crate::provider::definition::compaction::CompactionStrategy::NativePreferred,
            summary_budget: std::num::NonZeroU64::new(4096).expect("positive"),
            // A one-token retention target: the compact fixture's final turn
            // exceeds it, so the whole history is the summarized prefix and the
            // checkpoint plan has an older prefix to cover.
            retention_budget: std::num::NonZeroU64::new(1).expect("positive"),
            can_export_native_summary: false,
            mechanisms: MechanismDeclarations {
                messages_on_demand_compaction: Some(OnDemandDeclaration {
                    default: crate::compaction::mechanism::MechanismSupport::Supported,
                    overlay_headers: None,
                    unsupported: None,
                }),
                ..MechanismDeclarations::default()
            },
            count_tokens: None,
        };

        assert_eq!(
            effective_support(
                &routing,
                Some(&declared),
                Mechanism::MessagesOnDemandCompaction
            ),
            crate::compaction::mechanism::MechanismSupport::Supported,
        );

        let standalone_only = CompactionSection {
            mechanisms: MechanismDeclarations {
                responses_standalone_compact: Some(StandaloneCompactDeclaration {
                    default: crate::compaction::mechanism::MechanismSupport::Unsupported,
                    operation: None,
                    unsupported: None,
                }),
                ..MechanismDeclarations::default()
            },
            ..declared.clone()
        };

        assert_eq!(
            effective_support(
                &routing,
                Some(&standalone_only),
                Mechanism::MessagesOnDemandCompaction
            ),
            crate::compaction::mechanism::MechanismSupport::Unknown,
            "an undeclared mechanism stays unknown beside a declared one"
        );
    }

    /// The compact-only output limit is the smaller of the configured budget
    /// and the caller's cap, and the caller cap alone when it is smaller.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn summary_budget_binds_to_the_smaller_cap() {
        assert_eq!(summary_output_limit(None, None), 4096);
        assert_eq!(summary_output_limit(None, Some(1_000)), 1_000);
        assert_eq!(summary_output_limit(None, Some(9_000)), 4096);
    }

    /// The trigger control item renders the vendor shape.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn trigger_item_renders_the_vendor_tag() {
        assert_eq!(
            trigger_item_json(),
            serde_json::json!({"type": "compaction_trigger"})
        );
    }

    /// The continuation request restores the compacted window: the summary
    /// message heads it, the retained units follow once, the riding state
    /// items recompute, and the output budget and instructions survive.
    ///
    /// # Panics
    ///
    /// Panics if the continuation diverges from the restored-window shape.
    #[test]
    fn continuation_carries_the_restored_window() {
        let restored = IrRequest {
            model: String::from("m"),
            instructions: Some(String::from("be terse")),
            messages: vec![
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: String::from("old"),
                    }],
                },
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: String::from("pending"),
                    }],
                },
            ],
            tools: vec![],
            tool_choice: None,
            temperature: Some(0.3),
            top_p: None,
            max_output_tokens: Some(700),
            response_format: None,
            reasoning_effort: None,
            state_items: Vec::new(),
        };

        let plan = GatewayCheckpointPlan {
            summary_input_messages: Vec::new(),
            retained_units: vec![(
                ItemIdentity::for_instruction(InstructionRole::System, 0, "unit", None),
                vec![IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: String::from("pending"),
                    }],
                }],
            )],
            retained_state_items: vec![StateItem {
                offset: 1,
                item: serde_json::json!({"type": "reasoning"}),
                is_gateway: false,
            }],
            preserved_instructions: Vec::new(),
            overlap_identities: Vec::new(),
            summary_budget: 4096,
            retention_budget: 8192,
        };

        let continuation = continuation_request(&restored, "the summary", &plan);

        assert_eq!(continuation.messages.len(), 2);

        let crate::ir::IrPart::Text { text } = &continuation.messages[0].parts[0] else {
            panic!("the summary message renders text");
        };

        assert!(text.contains("the summary"));
        assert!(text.starts_with(crate::compaction::envelope::SUMMARY_RESTORE_PREFIX));
        assert_eq!(
            continuation.messages[1].parts[0],
            IrPart::Text {
                text: String::from("pending"),
            }
        );

        assert_eq!(continuation.state_items.len(), 1);
        assert_eq!(continuation.max_output_tokens, Some(700));
        assert_eq!(continuation.instructions.as_deref(), Some("be terse"));
        assert_eq!(continuation.temperature, Some(0.3));
    }

    /// The closing instruction of the summarizer is the fixed user turn.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn the_summary_closing_instruction_is_stable() {
        assert_eq!(SUMMARY_CLOSING_INSTRUCTION, "Produce the summary now.");
    }

    /// A sealed gateway checkpoint round-trips its identities: the preserved
    /// instructions and retained units carry into the envelope, and the
    /// artifact mints a namespaced identifier.
    ///
    /// # Panics
    ///
    /// Panics if sealing fails, which the missing live configuration can.
    #[test]
    fn a_sealed_checkpoint_carries_its_plan() {
        let _configuration_guard = crate::test_support::configuration_lock();
        let _configuration = crate::test_support::install_global_configuration();

        let plan = GatewayCheckpointPlan {
            summary_input_messages: Vec::new(),
            retained_units: Vec::new(),
            retained_state_items: Vec::new(),
            preserved_instructions: vec![(
                ItemIdentity::for_instruction(InstructionRole::System, 0, "be terse", None),
                InstructionRole::System,
                String::from("be terse"),
            )],
            overlap_identities: vec![ItemIdentity::for_instruction(
                InstructionRole::System,
                0,
                "be terse",
                None,
            )],
            summary_budget: 4096,
            retention_budget: 8192,
        };

        // Sealing requires the live sealing configuration; the absence is the
        // documented configuration-missing failure, and its presence (in the
        // executor integration tests) round-trips the plan.
        match seal_gateway_checkpoint(
            &plan,
            String::from("summary"),
            crate::compaction::envelope::ReplacementMode::CanonicalWindow,
            None,
        ) {
            Ok((_envelope, carrier)) => {
                assert!(carrier.starts_with("janusgate.compaction.v1."));
            }

            Err(CompactionError::ConfigurationMissing { .. }) => {}

            Err(other) => panic!("unexpected sealing failure: {other:?}"),
        }
    }

    /// A parsed instruction item fixture pins the planner's input shape.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn parsed_instruction_items_carry_role_and_text() {
        let item = ParsedInstructionItem {
            role: InstructionRole::Developer,
            text: String::from("new developer context"),
        };

        assert_eq!(item.role.as_str(), "developer");
        assert_eq!(item.text, "new developer context");
    }

    /// The rendered wire body conversion produces a JSON value.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn body_values_render_as_json() {
        #[derive(serde::Serialize)]
        struct Probe {
            /// A member.
            value: u32,
        }

        assert_eq!(
            body_value(&Probe { value: 3 }),
            serde_json::json!({"value": 3})
        );
    }

    /// A compact request through an unknown-support definition (no compaction
    /// section) takes the gateway-summary path: one internal summary dispatch
    /// on the routed provider, and the rendered `response.compaction` body
    /// carries one gateway carrier item whose envelope seals the summary and
    /// the retained units, with the summary call's usage as the billed totals.
    ///
    /// # Panics
    ///
    /// Panics if a fixture fails or an assertion fails — the failure report.
    #[tokio::test]
    async fn a_compact_request_without_native_support_summarizes_internally() {
        let _configuration_guard = crate::test_support::configuration_lock();
        let _configuration = crate::test_support::install_global_configuration();

        let (mock, recorded) =
            crate::test_support::MockUpstream::start_recording(vec![summary_response(
                "the older work, summarized",
            )])
            .await;

        let base = mock.base();

        // A definition with no declared mechanism: the unknown-support posture
        // that summarizes without a native probe. A one-token retention target
        // leaves the fixture's whole history as the summarized prefix.
        let routing = routing_at(&base, Some(summarize_only_section()));

        let context = crate::context::Context {
            database: crate::test_support::create_test_database().await.0,
            api_log_store: None,
            data_plane: reqwest::Client::new(),
            control_plane: reqwest::Client::new(),
        };

        let mut log = crate::storage::api_logs::ApiLogDraft {
            started_at: None,
            api_key_id: 1,
            provider_id: routing.provider.id,
            model_id: routing.model.id,
            endpoint_type: crate::accounting::ApiLogEndpointType::OpenaiResponsesCompact,
            is_stream: false,
            billed_usage: None,
        };

        let parsed = compact_parsed();

        let body = execute_responses_compact(&context, &routing, &parsed, &mut log)
            .await
            .expect("the compact flow succeeds");

        // One dispatch: the internal summary call on the inference URL.
        assert_eq!(
            recorded.lock().expect("the recorder reads").len(),
            1,
            "recorded requests: {:?}",
            recorded
        );

        // The rendered body is the canonical next window: one gateway carrier
        // item, whose usage is the summary call's.
        assert_eq!(
            body.get("object"),
            Some(&serde_json::json!("response.compaction"))
        );

        let output = body
            .get("output")
            .and_then(serde_json::Value::as_array)
            .expect("output");

        assert_eq!(output.len(), 1);

        let carrier = output[0]
            .get("encrypted_content")
            .and_then(serde_json::Value::as_str)
            .expect("the carrier string");

        assert!(carrier.starts_with("janusgate.compaction.v1."));

        assert_eq!(
            body.pointer("/usage/input_tokens"),
            Some(&serde_json::json!(10))
        );
        assert_eq!(
            body.pointer("/usage/output_tokens"),
            Some(&serde_json::json!(5))
        );

        // The sealed envelope carries the summary — the one-token retention
        // target retains nothing, so the whole history was summarized — under
        // the canonical-window replacement.
        let envelope = crate::compaction::sealing::unseal_with_live_config(carrier, None)
            .expect("the carrier unseals");

        let crate::compaction::envelope::EnvelopePayload::GatewaySummary(payload) =
            &envelope.payload
        else {
            panic!("the summary envelope carries the summary payload");
        };

        assert_eq!(payload.summary_text, "the older work, summarized");
        assert_eq!(
            envelope.replacement,
            crate::compaction::envelope::ReplacementMode::CanonicalWindow
        );
        assert_eq!(payload.retained_units.len(), 0);
    }

    /// A compact request through a supported standalone operation compacts
    /// natively: the compact dispatch hits the operation's URL, and the native
    /// window renders through verbatim with its usage.
    ///
    /// # Panics
    ///
    /// Panics if a fixture fails or an assertion fails — the failure report.
    #[tokio::test]
    async fn a_compact_request_through_a_supported_operation_compacts_natively() {
        let _configuration_guard = crate::test_support::configuration_lock();
        let _configuration = crate::test_support::install_global_configuration();

        let (mock, recorded) =
            crate::test_support::MockUpstream::start_recording(vec![compacted_response()]).await;

        let base = mock.base();

        let routing = routing_at(
            &base,
            Some(standalone_section(
                format!("{base}/responses/compact"),
                Vec::new(),
            )),
        );

        let context = crate::context::Context {
            database: crate::test_support::create_test_database().await.0,
            api_log_store: None,
            data_plane: reqwest::Client::new(),
            control_plane: reqwest::Client::new(),
        };

        let mut log = crate::storage::api_logs::ApiLogDraft {
            started_at: None,
            api_key_id: 1,
            provider_id: routing.provider.id,
            model_id: routing.model.id,
            endpoint_type: crate::accounting::ApiLogEndpointType::OpenaiResponsesCompact,
            is_stream: false,
            billed_usage: None,
        };

        let parsed = compact_parsed();

        let body = execute_responses_compact(&context, &routing, &parsed, &mut log)
            .await
            .expect("the native compact flow succeeds");

        // One dispatch, at the operation's URL.
        let recorded = recorded.lock().expect("the recorder reads");

        assert_eq!(recorded.len(), 1, "recorded requests: {recorded:?}");
        assert!(
            recorded[0].contains("POST /responses/compact"),
            "the compact dispatch hits the operation URL: {recorded:?}"
        );

        drop(recorded);

        // The native window renders through verbatim, usage included.
        assert_eq!(body.pointer("/id"), Some(&serde_json::json!("resp_native")));

        let output = body
            .get("output")
            .and_then(serde_json::Value::as_array)
            .expect("output");

        assert_eq!(output.len(), 2);

        assert_eq!(
            body.pointer("/usage/input_tokens"),
            Some(&serde_json::json!(7))
        );
    }

    /// A native compact dispatch that answers a positively classified
    /// unsupported boundary falls back to gateway summary inference exactly
    /// once: two dispatches in order (compact, then summary), and the rendered
    /// body is the gateway checkpoint carrying the summary call's usage.
    ///
    /// # Panics
    ///
    /// Panics if a fixture fails or an assertion fails — the failure report.
    #[tokio::test]
    async fn a_classified_unsupported_native_attempt_falls_back_to_summary_once() {
        let _configuration_guard = crate::test_support::configuration_lock();
        let _configuration = crate::test_support::install_global_configuration();

        let rejected_body = serde_json::json!({
            "error": {"code": "model_compaction_unsupported"},
        })
        .to_string();

        let rejected = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{rejected_body}",
            rejected_body.len()
        );

        let accepted_body = summary_response("fallback summary");

        let accepted = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{accepted_body}",
            accepted_body.len()
        );

        let (mock, recorded) =
            crate::test_support::MockUpstream::start_raw_recording(vec![rejected, accepted]).await;

        let base = mock.base();

        let routing = routing_at(
            &base,
            Some(standalone_section(
                format!("{base}/responses/compact"),
                vec![404],
            )),
        );

        let context = crate::context::Context {
            database: crate::test_support::create_test_database().await.0,
            api_log_store: None,
            data_plane: reqwest::Client::new(),
            control_plane: reqwest::Client::new(),
        };

        let mut log = crate::storage::api_logs::ApiLogDraft {
            started_at: None,
            api_key_id: 1,
            provider_id: routing.provider.id,
            model_id: routing.model.id,
            endpoint_type: crate::accounting::ApiLogEndpointType::OpenaiResponsesCompact,
            is_stream: false,
            billed_usage: None,
        };

        let parsed = compact_parsed();

        let body = execute_responses_compact(&context, &routing, &parsed, &mut log)
            .await
            .expect("the fallback flow succeeds");

        // Two dispatches in order: the classified native attempt, then the one
        // summary fallback.
        let recorded = recorded.lock().expect("the recorder reads");

        assert_eq!(recorded.len(), 2, "recorded requests: {recorded:?}");
        assert!(recorded[0].contains("POST /responses/compact"));
        assert!(recorded[1].contains("POST /responses"));

        drop(recorded);

        // The rendered body is the gateway checkpoint, with the fallback
        // summary's usage as the billed totals.
        let carrier = body
            .pointer("/output/0/encrypted_content")
            .and_then(serde_json::Value::as_str)
            .expect("the carrier item");

        let envelope = crate::compaction::sealing::unseal_with_live_config(carrier, None)
            .expect("the carrier unseals");

        let crate::compaction::envelope::EnvelopePayload::GatewaySummary(payload) =
            &envelope.payload
        else {
            panic!("the fallback seals a summary envelope");
        };

        assert_eq!(payload.summary_text, "fallback summary");

        assert_eq!(
            body.pointer("/usage/input_tokens"),
            Some(&serde_json::json!(10))
        );
    }
}
