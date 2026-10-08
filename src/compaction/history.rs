//! History materialization (README §6.8): the restoration of the authenticated
//! active window before every normal inference and re-compaction, the gateway
//! checkpoint planner, and the sealing of gateway envelopes into carriers.
//!
//! The boundary semantics are authenticated before account selection in
//! [`crate::compaction_model::boundary`]; this module materializes them: the
//! latest authenticated usable checkpoint supersedes the covered history —
//! conversational entries wholesale, instruction entries by proven identity
//! overlap — while an unmatched current instruction survives. A null/no-op
//! native block does not establish a boundary. A recognizable gateway carrier
//! is decoded and verified or fails the request explicitly; a damaged marker is
//! never forwarded upstream as native state and never summarized.

use super::error::CompactionError;
use crate::{
    compaction_model::{
        boundary::{ActiveBoundary, BoundaryPreparation},
        envelope::{
            EnvelopePayload, EnvelopeV1, GatewaySummaryPayload, NativeReplayWindow,
            NativeWindowPayload, PrefixIdentity, PreservedInstruction, ReplacementMode,
            RetainedUnit, summary_message,
        },
        identity::{ItemIdentity, hex_sha256},
        sealing,
    },
    format::{NativeArtifact, ParsedCompaction, UpstreamFormat},
    ids,
    ir::{IrMessage, IrRequest, IrRole, StateFamily, StateItem},
};
use serde::Deserialize as _;
use serde_json::Value;

/// The restored active boundary of a prepared request: the checkpoint the
/// window was reconstructed from, when one stands.
#[derive(Debug, Clone, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "one transient boundary is held per request; boxing adds a carrier allocation"
)]
pub(super) enum RestoredBoundary {
    /// A decoded gateway checkpoint.
    Gateway {
        /// The decoded envelope.
        envelope: EnvelopeV1,
    },

    /// Native compaction state replayed to a compatible producer; the gateway
    /// performed no restoration, the upstream interprets the state.
    Native(NativeArtifact),
}

/// The gateway budgets of one routed request (README §5, §6.8): the
/// definition's stated budgets, or the shipped defaults when the definition
/// carries no compaction section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Budgets {
    /// The summary output budget in tokens.
    pub(super) summary: u64,

    /// The retained-input soft target in tokens.
    pub(super) retention: u64,
}

/// The instruction items a checkpoint plan covers (README §6.8).
pub(super) enum CheckpointInstructions<'a> {
    /// The request's parsed instruction items. The client codecs join exactly
    /// these items into the request's instructions string, so the fresh
    /// occurrence-indexed identities computed over them are the identities a
    /// later replay computes over its own prefix.
    Parsed(&'a [crate::format::ParsedInstructionItem]),

    /// Per-item continuity handed forward by a restored gateway summary: the
    /// payload's preserved items under their sealed identities, then the
    /// unmatched current items under their replay identities. A re-compaction
    /// over such a window plans with these, so the new checkpoint's coverage
    /// identities stay matchable by later replays; planning from the request's
    /// parsed items instead would seal one joined identity that can never
    /// overlap-match and would duplicate the instruction context.
    Restored(Vec<PreservedInstruction>),
}

/// Materializes the already authenticated active boundary using the existing replacement rules.
/// Superseded gateway carriers are removed without exporting their hidden content.
///
/// Returns the restored boundary and the instruction items a checkpoint plan
/// covers: a restored gateway summary hands forward its per-item continuity,
/// every other shape keeps the request's parsed items authoritative.
///
/// # Errors
///
/// Returns an invalid replacement boundary or target-incompatible native-window failure.
fn restore<'a>(
    ir: &mut IrRequest,
    parsed: &'a ParsedCompaction,
    preparation: &BoundaryPreparation,
    target_format: UpstreamFormat,
) -> Result<(Option<RestoredBoundary>, CheckpointInstructions<'a>), CompactionError> {
    let Some(active) = preparation.active() else {
        remove_gateway_state_items(ir);
        return Ok((
            None,
            CheckpointInstructions::Parsed(&parsed.instruction_items),
        ));
    };

    // The active boundary's own state slot orders it among the state items
    // sharing its message offset: state the boundary's encounter order
    // precedes is covered, state that follows rides the fresh suffix
    // (README §6.8's supersession rule).
    let slot = preparation.active_state_slot(ir);

    match active {
        ActiveBoundary::Gateway { offset, envelope } => {
            match &envelope.payload {
                EnvelopePayload::GatewaySummary(payload) => {
                    let instructions = restore_gateway_summary(ir, parsed, offset, slot, payload)?;
                    return Ok((
                        Some(RestoredBoundary::Gateway {
                            envelope: envelope.clone(),
                        }),
                        CheckpointInstructions::Restored(instructions),
                    ));
                }
                EnvelopePayload::NativeWindow(payload) => {
                    restore_native_window(ir, offset, slot, payload, target_format)?;
                }
            }
            Ok((
                Some(RestoredBoundary::Gateway {
                    envelope: envelope.clone(),
                }),
                CheckpointInstructions::Parsed(&parsed.instruction_items),
            ))
        }
        ActiveBoundary::Native { artifact, .. } => {
            remove_gateway_state_items(ir);
            Ok((
                Some(RestoredBoundary::Native(artifact)),
                CheckpointInstructions::Parsed(&parsed.instruction_items),
            ))
        }
    }
}

/// Prepares authenticated state against the selected producer, permitting only explicit
/// readable-summary export. `preparation` must belong to this same parsed request.
///
/// An authorized export rebuilds the window losslessly: the readable summary
/// heads it, the payload's retained messages follow, and the fresh suffix —
/// messages and opaque state alike — rides at its recomputed offset, exactly
/// as [`restore`] places it. State the boundary's own `(offset, slot)`
/// position precedes is covered and superseded; unreadable native state fails
/// the export explicitly.
///
/// Returns the restored boundary and the instruction items a checkpoint plan
/// covers, as [`restore`] documents.
///
/// # Errors
///
/// Returns a producer, model, format, retained-state, or replacement-boundary incompatibility.
pub(super) fn prepare_for_target<'a>(
    ir: &mut IrRequest,
    parsed: &'a ParsedCompaction,
    routing: &crate::provider::ModelRouting,
    preparation: &BoundaryPreparation,
) -> Result<(Option<RestoredBoundary>, CheckpointInstructions<'a>), CompactionError> {
    let target_format = routing.provider.definition.payload.format;
    let can_export = routing
        .provider
        .definition
        .payload
        .compaction
        .as_ref()
        .is_some_and(|section| section.can_export_native_summary);
    let mut exported_retained = Vec::new();
    if let Some(active) = preparation.active() {
        // The active boundary's own state slot orders it among the state items
        // sharing its message offset: same-offset state the boundary's
        // encounter order precedes is covered with the prefix, state that
        // follows it is the fresh suffix (README §6.8's supersession rule).
        let slot = preparation.active_state_slot(ir);
        let mut exported = None;
        match active {
            ActiveBoundary::Gateway { envelope, .. } => {
                if let EnvelopePayload::NativeWindow(payload) = &envelope.payload {
                    let is_compatible = payload.producer_instance_id
                        == NativeWindowPayload::producer_instance(routing.provider.id)
                        && payload.upstream_model_id == routing.model.upstream_id
                        && payload.native_format == target_format;
                    if !is_compatible {
                        if can_export
                            && payload.retained_state_items.is_empty()
                            && let NativeReplayWindow::AnthropicOnDemandBlock { block } =
                                &payload.window
                        {
                            exported = readable_native_summary(block);
                            exported_retained = payload.retained_messages.clone();
                        }
                        if exported.is_none() {
                            return Err(CompactionError::inaccessible_native_state(
                                "the wrapped native state belongs to another provider or model",
                            ));
                        }
                    }
                }
            }
            ActiveBoundary::Native {
                artifact: NativeArtifact::MessagesCompactionBlock { .. },
                ..
            } if target_format != UpstreamFormat::AnthropicMessages && can_export => {
                // The boundary's own state item is the compaction block: probe
                // it by slot. An unrelated state item sharing the offset —
                // ahead of the boundary in encounter order — is covered state,
                // never the block.
                #[expect(
                    clippy::indexing_slicing,
                    reason = "active_state_slot panics unless the parser recorded the boundary's \
                              own state-item index, so the index is in bounds"
                )]
                let block = &ir.state_items[slot].item;
                exported = readable_native_summary(block);
            }
            _ => {}
        }
        if let Some(summary) = exported {
            let offset = match active {
                ActiveBoundary::Gateway { offset, .. } | ActiveBoundary::Native { offset, .. } => {
                    offset
                }
            };
            if ir.messages.iter().skip(offset).any(|message| {
                message
                    .parts
                    .iter()
                    .any(|part| matches!(part, crate::ir::IrPart::Reasoning { .. }))
            }) {
                return Err(CompactionError::inaccessible_native_state(
                    "readable export cannot preserve signed reasoning",
                ));
            }
            if offset > ir.messages.len() {
                return Err(CompactionError::invalid_gateway_state(
                    "the checkpoint boundary exceeds the request history",
                ));
            }
            if exported_retained.iter().any(|message| {
                message
                    .parts
                    .iter()
                    .any(|part| matches!(part, crate::ir::IrPart::Reasoning { .. }))
            }) {
                return Err(CompactionError::inaccessible_native_state(
                    "readable export cannot preserve signed reasoning",
                ));
            }
            let fresh = ir.messages.split_off(offset);
            // The exported window: one summary message, then the payload's
            // retained messages, then the fresh suffix.
            let leading_state_offset = 1 + exported_retained.len();
            ir.messages = vec![summary_message(&summary)];
            ir.messages.extend(exported_retained);
            ir.messages.extend(fresh);
            // Fresh state rides the export at its recomputed offset — state
            // strictly after the boundary's own (offset, slot) position,
            // exactly as the restore paths treat it, so the export stays
            // lossless for representable state. Same-offset state the boundary
            // precedes is covered with the prefix and superseded; gateway items
            // are dropped. The export never clears past the covered prefix.
            ir.state_items = ir
                .state_items
                .iter()
                .enumerate()
                .filter(|(index, item)| {
                    !item.is_gateway
                        && (item.offset > offset || (item.offset == offset && *index > slot))
                })
                .map(|(_index, item)| StateItem {
                    offset: leading_state_offset + (item.offset - offset),
                    item: item.item.clone(),
                    is_gateway: false,
                    is_compaction: item.is_compaction,
                    family: item.family,
                })
                .collect();
            return Ok((
                None,
                CheckpointInstructions::Parsed(&parsed.instruction_items),
            ));
        }
    }
    restore(ir, parsed, preparation, target_format)
}

/// Reads only a native block whose entire state can be represented by readable text.
fn readable_native_summary(block: &Value) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct ReadableBlock {
        /// Readable summary text.
        content: Option<String>,
        /// Ciphertext cannot be exported as text.
        #[serde(default)]
        encrypted_content: Option<String>,
        /// Durable tool changes cannot be discarded.
        #[serde(default)]
        tool_changes: Option<Vec<Value>>,
    }
    let block = ReadableBlock::deserialize(block).ok()?;
    if block.encrypted_content.is_some() || block.tool_changes.is_some() {
        return None;
    }
    block.content.filter(|text| !text.trim().is_empty())
}

/// Removes every gateway-flagged state item, leaving native state untouched.
fn remove_gateway_state_items(ir: &mut IrRequest) {
    ir.state_items.retain(|item| !item.is_gateway);
}

/// Restores a gateway summary checkpoint: supersedes the covered conversational
/// prefix, drops the covered instruction items by proven identity overlap,
/// rebuilds the instruction context, and restores the summary and the retained
/// units once ahead of the fresh suffix.
///
/// `slot` is the boundary's own state-item index: same-offset state the
/// boundary's encounter order precedes is covered with the prefix, same-offset
/// state after it rides the fresh suffix.
///
/// Returns the restored window's instruction items — the payload's preserved
/// items under their sealed identities, then the unmatched current items under
/// their replay identities — so a re-compaction plan over this window keeps
/// identity continuity instead of collapsing the instructions into one joined
/// identity no replay could match.
fn restore_gateway_summary(
    ir: &mut IrRequest,
    parsed: &ParsedCompaction,
    offset: usize,
    slot: usize,
    payload: &GatewaySummaryPayload,
) -> Result<Vec<PreservedInstruction>, CompactionError> {
    if offset > ir.messages.len() {
        return Err(CompactionError::invalid_gateway_state(
            "the gateway checkpoint's boundary sits beyond the request's history",
        ));
    }

    let fresh: Vec<IrMessage> = ir.messages.split_off(offset);

    // The identity pass: a current instruction item proven to overlap the
    // checkpoint's covered identities is superseded; an unmatched one — a new
    // developer instruction, for instance — is preserved.
    let identities = ItemIdentity::for_instruction_window(
        parsed
            .instruction_items
            .iter()
            .map(|item| (item.role, item.text.as_str())),
    );

    let preserved_current: Vec<(&crate::format::ParsedInstructionItem, ItemIdentity)> = parsed
        .instruction_items
        .iter()
        .zip(identities)
        .filter(|(_item, identity)| {
            !payload
                .overlap_identities
                .iter()
                .any(|covered| covered.matches(identity))
        })
        .collect();

    // The restored window's instruction items: the payload's preserved items
    // ride their sealed identities first, then the preserved current items.
    let mut restored_instructions: Vec<PreservedInstruction> =
        payload.preserved_instructions.clone();

    restored_instructions.extend(preserved_current.iter().map(|(item, identity)| {
        PreservedInstruction {
            identity: identity.clone(),
            role: item.role,
            text: item.text.clone(),
        }
    }));

    // Rebuild the instruction context: the checkpoint's preserved instructions
    // first, then the current instructions that survived the overlap pass.
    let mut instruction_parts: Vec<String> = payload
        .preserved_instructions
        .iter()
        .map(|instruction| instruction.text.clone())
        .collect();

    instruction_parts.extend(preserved_current.iter().map(|(item, _)| item.text.clone()));

    ir.instructions = (!instruction_parts.is_empty()).then(|| instruction_parts.join("\n\n"));

    // Restore once: the summary message, then the retained units' messages,
    // then the fresh suffix.
    let mut messages = Vec::with_capacity(1 + payload.retained_units.len() + fresh.len());

    messages.push(summary_message(&payload.summary_text));

    for unit in &payload.retained_units {
        messages.extend(unit.messages.iter().cloned());
    }

    let retained_message_count = messages.len() - 1;

    messages.extend(fresh.iter().cloned());

    ir.messages = messages;

    // Keep the fresh suffix's native state at its recomputed position: one
    // summary message plus the retained messages now precede it. Fresh means
    // strictly after the boundary's own (offset, slot) position — same-offset
    // state ahead of the boundary is covered with the prefix.
    let fresh_state_items: Vec<StateItem> = ir
        .state_items
        .iter()
        .enumerate()
        .filter(|(index, item)| {
            !item.is_gateway && (item.offset > offset || (item.offset == offset && *index > slot))
        })
        .map(|(_index, item)| StateItem {
            offset: 1 + retained_message_count + (item.offset - offset),
            item: item.item.clone(),
            is_gateway: false,
            is_compaction: item.is_compaction,
            family: item.family,
        })
        .collect();

    ir.state_items = payload.retained_state_items.clone();
    ir.state_items.extend(fresh_state_items);

    Ok(restored_instructions)
}

/// Computes the prefix identity of one request: digests of its system
/// instructions and tool definitions — the signed-state-binding surface a
/// producer verifies. Recorded on the wrap side and compared on replay
/// (README §6.8's `required_prefix_identity`).
///
/// # Panics
///
/// Panics if serializing the tool definitions fails — impossible for
/// concrete IR tools.
#[must_use]
pub(super) fn produced_prefix_identity(ir: &IrRequest) -> PrefixIdentity {
    let instructions_digest = ir
        .instructions
        .as_deref()
        .map(|instructions| hex_sha256(instructions.as_bytes()));

    let tools_digest = if ir.tools.is_empty() {
        None
    } else {
        #[expect(
            clippy::expect_used,
            reason = "serializing concrete IR tools cannot fail"
        )]
        let canonical = serde_json::to_vec(&ir.tools).expect("the tool definitions serialize");

        Some(hex_sha256(&canonical))
    };

    PrefixIdentity {
        instructions_digest,
        tools_digest,
    }
}

/// Restores a wrapped native window: drops the covered prefix and places the
/// exact native artifact at the boundary — a whole Responses window replayed
/// as state items, or an Anthropic on-demand block at the leading placement
/// its replay contract demands (cross-protocol continuation never applies the
/// source protocol's historical placement to the target payload).
///
/// `slot` is the boundary's own state-item index: same-offset state the
/// boundary's encounter order precedes is covered with the prefix, same-offset
/// state after it rides the fresh suffix.
fn restore_native_window(
    ir: &mut IrRequest,
    offset: usize,
    slot: usize,
    payload: &NativeWindowPayload,
    target_format: UpstreamFormat,
) -> Result<(), CompactionError> {
    // Wrapping does not enable cross-vendor native conversion: the window
    // replays only against its own wire format.
    if payload.native_format != target_format {
        return Err(CompactionError::inaccessible_native_state(
            "the wrapped native window speaks another wire format than the selected target",
        ));
    }

    // A recorded prefix requirement is the producer's signature-binding
    // contract: replaying under a changed system/tool prefix would hand the
    // producer state it must reject, so the mismatch fails explicitly here.
    if let Some(required) = &payload.required_prefix_identity {
        let current = produced_prefix_identity(ir);

        let instructions_match = required.instructions_digest == current.instructions_digest;

        let tools_match = required.tools_digest == current.tools_digest;

        if !instructions_match || !tools_match {
            return Err(CompactionError::inaccessible_native_state(
                "the wrapped native state requires the producer's system/tool prefix, \
                 which this request's context does not match",
            ));
        }
    }

    if offset > ir.messages.len() {
        return Err(CompactionError::invalid_gateway_state(
            "the gateway checkpoint's boundary sits beyond the request's history",
        ));
    }

    let fresh: Vec<IrMessage> = ir.messages.split_off(offset);

    // Native state of the covered prefix is superseded by the wrapped window;
    // state strictly after the boundary's own (offset, slot) position rides at
    // its recomputed position — same-offset state ahead of the boundary is
    // covered with the prefix.
    let fresh_state_items: Vec<StateItem> = ir
        .state_items
        .iter()
        .enumerate()
        .filter(|(index, item)| {
            !item.is_gateway && (item.offset > offset || (item.offset == offset && *index > slot))
        })
        .map(|(_index, item)| StateItem {
            offset: payload.retained_messages.len() + item.offset - offset,
            item: item.item.clone(),
            is_gateway: false,
            is_compaction: item.is_compaction,
            family: item.family,
        })
        .collect();

    let mut restored_state_items = Vec::new();

    match &payload.window {
        NativeReplayWindow::ResponsesWindow { items } => {
            // The whole window replays as one block at the boundary: the render
            // splices same-offset state items in their vector order. The
            // window's items are the producer's own compaction state.
            restored_state_items.extend(items.iter().map(|item| StateItem {
                offset: 0,
                item: item.clone(),
                is_gateway: false,
                is_compaction: true,
                family: StateFamily::Responses,
            }));
        }

        NativeReplayWindow::AnthropicOnDemandBlock { block } => {
            // Leading placement: the block opens the restored window and the
            // fresh suffix follows once. The block is the producer's own
            // compaction state.
            restored_state_items.push(StateItem {
                offset: 0,
                item: block.clone(),
                is_gateway: false,
                is_compaction: true,
                family: StateFamily::Messages,
            });
        }
    }

    restored_state_items.extend(payload.retained_state_items.clone());
    restored_state_items.extend(fresh_state_items);

    ir.messages = payload.retained_messages.clone();
    ir.messages.extend(fresh);
    ir.state_items = restored_state_items;

    Ok(())
}

/// Restores a freshly produced native window for its immediate continuation.
///
/// The produced window postdates every state item the request carried, so the
/// boundary slot sits after all of them: no boundary-offset state rides the
/// fresh filter, and the window's own retained state — the payload's — is the
/// only copy that restores.
pub(super) fn native_continuation(
    ir: &IrRequest,
    payload: &NativeWindowPayload,
) -> Result<IrRequest, CompactionError> {
    let mut continuation = ir.clone();
    let boundary = continuation.messages.len();
    let slot = continuation.state_items.len();
    restore_native_window(
        &mut continuation,
        boundary,
        slot,
        payload,
        payload.native_format,
    )?;
    Ok(continuation)
}

/// The gateway checkpoint plan (README §6.8): the eligible older prefix the
/// summary will cover, the retained interaction units and the native state
/// riding them, and the preserved instruction context with its coverage
/// identities.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct GatewayCheckpointPlan {
    /// The eligible older prefix the summary covers, as input history.
    /// Consumed by [`crate::compaction::summary::plan_summary_request`]; no
    /// later consumer of the plan reads it.
    pub(super) summary_input_messages: Vec<IrMessage>,

    /// The retained interaction units, in conversation order. Each unit is
    /// indivisible: whole tool exchanges stay together.
    pub(super) retained_units: Vec<Vec<IrMessage>>,

    /// The native state items positioned inside the retained region, with
    /// offsets recomputed against the restored window layout (one summary
    /// message, then the retained units' messages).
    pub(super) retained_state_items: Vec<StateItem>,

    /// The current instruction items with their identities and roles, in the
    /// envelope payload's own form.
    preserved_instructions: Vec<PreservedInstruction>,

    /// The identities of the preserved instruction items the checkpoint
    /// covers — replay's only matching surface, the covered conversation
    /// prefix superseding wholesale at the boundary offset.
    overlap_identities: Vec<ItemIdentity>,

    /// The summary output budget in tokens.
    pub(super) summary_budget: u64,

    /// The retained-input soft target in tokens.
    retention_budget: u64,
}

/// One interaction unit: an indivisible run of messages covering a turn and its
/// whole tool exchange.
struct InteractionUnit {
    /// The unit's messages.
    messages: Vec<IrMessage>,

    /// The message index the unit starts at.
    start: usize,
}

/// Plans a gateway checkpoint from the restored window (README §6.8): groups
/// the history into indivisible interaction units, retains recent complete
/// units backward under the retention budget — the mandatory active turn never
/// partially truncated — and leaves the older prefix for the summary.
///
/// `mandatory_active_turn` is set for compact-and-continue requests: the unit
/// containing the last user message, through the end of the messages, is
/// mandatory retained state. Compact-only requests may summarize a final user
/// turn.
///
/// # Errors
///
/// Returns [`CompactionError::UnusableHistory`] for an orphan tool result,
/// [`CompactionError::InaccessibleNativeState`] when the older prefix carries
/// opaque state the summary cannot faithfully represent, and
/// [`CompactionError::BudgetExceeded`] when retention leaves no older prefix to
/// summarize.
pub(super) fn plan_gateway_checkpoint(
    ir: &IrRequest,
    instructions: &CheckpointInstructions<'_>,
    mandatory_active_turn: bool,
    budgets: Budgets,
) -> Result<GatewayCheckpointPlan, CompactionError> {
    plan_checkpoint(ir, instructions, mandatory_active_turn, budgets, false)
}

/// Plans a native checkpoint without rejecting opaque state the native producer can consume.
pub(super) fn plan_native_checkpoint(
    ir: &IrRequest,
    instructions: &CheckpointInstructions<'_>,
    budgets: Budgets,
) -> Result<GatewayCheckpointPlan, CompactionError> {
    plan_checkpoint(ir, instructions, true, budgets, true)
}

/// Selects whole interaction units for a contiguous retained suffix.
fn plan_checkpoint(
    ir: &IrRequest,
    instructions: &CheckpointInstructions<'_>,
    mandatory_active_turn: bool,
    budgets: Budgets,
    can_compact_native_state: bool,
) -> Result<GatewayCheckpointPlan, CompactionError> {
    let units = interaction_units(&ir.messages)?;

    // The mandatory region is the run of units that contains the last user
    // message through the end of the messages; a compact-only request (no
    // mandatory active turn) mandates nothing.
    let mut mandatory_start_index = if mandatory_active_turn {
        let last_user = ir
            .messages
            .iter()
            .rposition(|message| message.role == IrRole::User);

        match last_user {
            // The index of the unit CONTAINING the last user message: the last
            // unit whose start index sits at or before it. Every unit from this
            // index on is mandatory.
            Some(position) => units
                .iter()
                .rposition(|unit| unit.start <= position)
                .unwrap_or(units.len()),

            None => units.len(),
        }
    } else {
        units.len()
    };

    // Images have no text estimate: the contiguous suffix from the earliest
    // image-carrying unit is retained rather than discarding modality data or
    // assigning it a fabricated size.
    let is_unit_multimodal: Vec<bool> = units
        .iter()
        .map(|unit| {
            unit.messages.iter().any(|message| {
                message
                    .parts
                    .iter()
                    .any(|part| matches!(part, crate::ir::IrPart::Image { .. }))
            })
        })
        .collect();

    if let Some(index) = is_unit_multimodal.iter().position(|is_image| *is_image) {
        mandatory_start_index = mandatory_start_index.min(index);
    }

    // Retention: recent complete units backward under the budget; mandatory
    // units always included — the budget is a soft target for mandatory state.
    let mut retained: Vec<&InteractionUnit> = Vec::new();

    let mut cumulative_tokens = 0u64;

    for (unit_index, (unit, is_multimodal)) in
        units.iter().zip(&is_unit_multimodal).enumerate().rev()
    {
        let unit_tokens = if *is_multimodal {
            0
        } else {
            super::threshold::estimate_message_tokens(&unit.messages)?
        };

        let is_mandatory = unit_index >= mandatory_start_index || *is_multimodal;

        if is_mandatory || cumulative_tokens + unit_tokens <= budgets.retention {
            cumulative_tokens += unit_tokens;
            retained.push(unit);
        } else {
            break;
        }
    }

    retained.reverse();

    // The retained units are a contiguous suffix of the units, so the first
    // one's start index is the message index the summarized prefix ends at.
    let first_retained_message = retained
        .first()
        .map(|unit| unit.start)
        .unwrap_or(ir.messages.len());

    // The older prefix is the contiguous leading run of units before the
    // retained suffix; if there is none, no useful compaction exists.
    let older_prefix: Vec<&InteractionUnit> = units
        .iter()
        .take_while(|unit| unit.start < first_retained_message)
        .collect();

    if older_prefix.is_empty() {
        return Err(CompactionError::budget_exceeded(
            "the retention budget and mandatory state leave no older history to summarize",
        ));
    }

    // Opaque native state inside the summarized prefix cannot be faithfully
    // summarized; state inside the retained region rides along.
    if !can_compact_native_state
        && ir
            .state_items
            .iter()
            .any(|item| !item.is_gateway && item.offset < first_retained_message)
    {
        return Err(CompactionError::inaccessible_native_state(
            "the history to summarize carries native state that cannot be summarized",
        ));
    }

    // State items inside the retained region, offsets recomputed against the
    // restored window: one summary message precedes the retained units, and
    // every retained message between the region start and the item's original
    // position precedes it there.
    let retained_state_items: Vec<StateItem> = ir
        .state_items
        .iter()
        .filter(|item| !item.is_gateway && item.offset >= first_retained_message)
        .map(|item| StateItem {
            offset: 1 + (item.offset - first_retained_message),
            item: item.item.clone(),
            is_gateway: false,
            is_compaction: item.is_compaction,
            family: item.family,
        })
        .collect();

    // The current instruction items with their identities: a restored
    // checkpoint's continuity rides its sealed identities, and parsed items
    // compute fresh occurrence-indexed identities — the codecs join exactly
    // the parsed items into the request's instructions string, so no other
    // divergence between the two exists to reconcile.
    let preserved_instructions: Vec<PreservedInstruction> = match instructions {
        CheckpointInstructions::Restored(items) => items.clone(),

        CheckpointInstructions::Parsed(items) => {
            let identities = ItemIdentity::for_instruction_window(
                items.iter().map(|item| (item.role, item.text.as_str())),
            );

            items
                .iter()
                .zip(identities)
                .map(|(item, identity)| PreservedInstruction {
                    identity,
                    role: item.role,
                    text: item.text.clone(),
                })
                .collect()
        }
    };

    let retained_units: Vec<Vec<IrMessage>> =
        retained.iter().map(|unit| unit.messages.clone()).collect();

    // The checkpoint's coverage identities are the preserved instructions
    // alone: replay matches only the instruction items, the covered
    // conversation prefix superseding wholesale at the boundary offset.
    let overlap_identities: Vec<ItemIdentity> = preserved_instructions
        .iter()
        .map(|instruction| instruction.identity.clone())
        .collect();

    Ok(GatewayCheckpointPlan {
        summary_input_messages: older_prefix
            .iter()
            .flat_map(|unit| unit.messages.iter().cloned())
            .collect(),
        retained_units,
        retained_state_items,
        preserved_instructions,
        overlap_identities,
        summary_budget: budgets.summary,
        retention_budget: budgets.retention,
    })
}

/// Groups the messages into indivisible interaction units: a new unit opens at
/// every user message and at an assistant message that does not follow a tool
/// result; tool results join the open unit, and an orphan tool result — one
/// with no open unit — is structurally invalid history.
///
/// # Panics
///
/// Panics if a tool result finds no open unit to join — impossible, the Tool
/// arm rejects an orphan before the join runs.
fn interaction_units(messages: &[IrMessage]) -> Result<Vec<InteractionUnit>, CompactionError> {
    let mut units: Vec<InteractionUnit> = Vec::new();

    for (index, message) in messages.iter().enumerate() {
        let opens_unit = match message.role {
            IrRole::User => true,

            IrRole::Assistant => {
                if index == 0 {
                    true
                } else {
                    #[expect(
                        clippy::indexing_slicing,
                        reason = "the index == 0 arm above guarantees index - 1 is in bounds"
                    )]
                    {
                        messages[index - 1].role != IrRole::Tool
                            && !(messages[index - 1].role == IrRole::Assistant
                                && messages[index - 1]
                                    .parts
                                    .iter()
                                    .any(|part| matches!(part, crate::ir::IrPart::ToolCall { .. })))
                    }
                }
            }

            IrRole::Tool => {
                if units.is_empty() {
                    return Err(CompactionError::unusable_history(
                        "the history opens with a tool result that answers no tool call",
                    ));
                }

                false
            }
        };

        if opens_unit {
            units.push(InteractionUnit {
                messages: vec![message.clone()],
                start: index,
            });
        } else {
            #[expect(
                clippy::expect_used,
                reason = "a tool result reaches this arm only when a unit is open, which the \
                          Tool arm above guarantees"
            )]
            let open = units
                .last_mut()
                .expect("a tool result joins the open interaction unit");

            open.messages.push(message.clone());
        }
    }

    Ok(units)
}

/// Seals a gateway envelope into its carrier string, binding the optional
/// visible content: the envelope's digest member and the AEAD binding both
/// carry the content's hex digest, so a carrier can never be consumed with
/// content other than what it authenticated.
///
/// # Errors
///
/// Returns [`CompactionError::ConfigurationMissing`] when no sealing keys are
/// configured; every other sealing failure is impossible over a constructed
/// envelope and surfaces as [`CompactionError::InvalidGatewayState`].
fn seal_envelope(
    payload: EnvelopePayload,
    replacement: ReplacementMode,
    visible_content: Option<&str>,
) -> Result<(EnvelopeV1, String), CompactionError> {
    let binding = visible_content.map(|content| hex_sha256(content.as_bytes()));

    let envelope = EnvelopeV1::new(
        format!("cmp_{}", ids::new_uuid()),
        replacement,
        payload,
        binding.clone(),
    );

    let carrier = sealing::seal_with_live_config(
        &envelope,
        binding.as_deref().map(|digest| digest.as_bytes()),
    )?;

    Ok((envelope, carrier))
}

/// Builds and seals the gateway summary checkpoint of one plan (README §6.8).
///
/// # Errors
///
/// Returns [`CompactionError`] as [`seal_envelope`].
pub(super) fn seal_gateway_checkpoint(
    plan: &GatewayCheckpointPlan,
    summary_text: String,
    replacement: ReplacementMode,
    visible_content: Option<&str>,
) -> Result<(EnvelopeV1, String), CompactionError> {
    let payload = EnvelopePayload::GatewaySummary(GatewaySummaryPayload {
        summary_text,
        preserved_instructions: plan.preserved_instructions.clone(),
        retained_units: plan
            .retained_units
            .iter()
            .map(|messages| RetainedUnit {
                messages: messages.clone(),
            })
            .collect(),
        retained_state_items: plan.retained_state_items.clone(),
        overlap_identities: plan.overlap_identities.clone(),
    });

    seal_envelope(payload, replacement, visible_content)
}

/// Builds and seals a wrapped native window checkpoint (README §6.8): the exact
/// native artifact beside the provenance needed to replay it against its
/// producer.
///
/// # Errors
///
/// Returns [`CompactionError`] as [`seal_envelope`].
pub(super) fn seal_native_window(
    payload: NativeWindowPayload,
    replacement: ReplacementMode,
    visible_content: Option<&str>,
) -> Result<(EnvelopeV1, String), CompactionError> {
    seal_envelope(
        EnvelopePayload::NativeWindow(payload),
        replacement,
        visible_content,
    )
}
