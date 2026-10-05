//! History materialization (README §6.8): the preparation pass that
//! verifies gateway carriers and reconstructs the active window before every
//! normal inference and re-compaction, the gateway checkpoint planner, and the
//! sealing of gateway envelopes into carriers.
//!
//! Preparation applies boundary semantics before role normalization: the
//! latest authenticated usable checkpoint supersedes the covered history —
//! conversational entries wholesale, instruction entries by proven identity
//! overlap — while an unmatched current instruction survives. A null/no-op
//! native block does not establish a boundary. A recognizable gateway carrier
//! is decoded and verified or fails the request explicitly; a damaged marker is
//! never forwarded upstream as native state and never summarized.

use super::{
    envelope::{
        EnvelopePayload, EnvelopeV1, GatewaySummaryPayload, NativeReplayWindow,
        NativeWindowPayload, PrefixIdentity, ReplacementMode, summary_message,
    },
    error::CompactionError,
    identity::{InstructionRole, ItemIdentity},
    sealing,
};
use crate::{
    format::{CompactionBoundary, NativeArtifact, ParsedCompaction, UpstreamFormat},
    ids,
    ir::{IrMessage, IrRequest, IrRole, StateItem},
};
use serde::Deserialize as _;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The restored active boundary of a prepared request: the checkpoint the
/// window was reconstructed from, when one stands.
#[derive(Debug, Clone, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "one transient boundary is held per request; boxing adds a carrier allocation"
)]
pub(crate) enum RestoredBoundary {
    /// A decoded gateway checkpoint.
    Gateway {
        /// The decoded envelope.
        envelope: EnvelopeV1,
    },

    /// Native compaction state replayed to a compatible producer; the gateway
    /// performed no restoration, the upstream interprets the state.
    Native(NativeArtifact),
}

/// Renders a SHA-256 digest as lowercase hex — the binding and identity digest
/// form every carrier comparison uses.
#[must_use]
pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(64);

    for byte in Sha256::digest(bytes) {
        rendered.push_str(&format!("{byte:02x}"));
    }

    rendered
}

/// Whether a history boundary is the native null/no-op block: it retains its
/// documented no-op meaning and does not establish a new boundary.
const fn is_null_no_op(boundary: &CompactionBoundary) -> bool {
    matches!(
        boundary,
        CompactionBoundary::Native {
            artifact: NativeArtifact::MessagesCompactionBlock {
                is_null_no_op: true,
                ..
            },
            ..
        }
    )
}

/// Target-independent authentication and latest usable boundary selection for one request.
/// It performs no account lookup, upstream I/O, or history materialization.
#[derive(Default)]
pub(crate) struct BoundaryPreparation {
    /// Latest authenticated non-no-op boundary; superseded decoded content is discarded.
    active: Option<PreparedBoundary>,
    /// Original boundary position, distinguishing multiple artifacts at one message offset.
    active_index: Option<usize>,
}

/// Content-free client-visible routing evidence prepared before account selection.
pub(crate) struct RoutingPreparation {
    /// Accepted active history, retaining its open tail for actual outward rendering.
    pub(crate) projector: crate::affinity::fingerprint::Projector,
    /// Latest authenticated carrier identity or exact canonical raw artifact identity.
    pub(crate) artifact: Option<crate::affinity::Digest>,
}

/// State discriminator used only to pair parsed boundary facts with their original state slots.
#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StateDiscriminator {
    /// Gateway or native compaction artifact.
    Compaction,
    /// Other opaque state, such as replayable encrypted reasoning.
    #[serde(other)]
    Other,
}

/// One validated active boundary, retaining only what subsequent restoration needs.
#[allow(
    clippy::large_enum_variant,
    reason = "one transient boundary is held per request; boxing adds a carrier allocation"
)]
enum PreparedBoundary {
    /// Authenticated gateway checkpoint at its original conversation offset.
    Gateway {
        /// Number of accepted conversation messages before this carrier.
        offset: usize,
        /// Already authenticated envelope, never unsealed again during this request.
        envelope: EnvelopeV1,
    },
    /// Raw producer-native state whose compatibility remains target-dependent.
    Native {
        /// Original conversation offset.
        offset: usize,
        /// Parsed native placement and no-op facts.
        artifact: NativeArtifact,
    },
}

impl BoundaryPreparation {
    /// Projects the validated client-visible active replay window without restoring hidden text.
    /// The latest carrier supersedes covered messages/state; no-op blocks establish no history
    /// unit or artifact hint. Original source records remain aligned with uncovered tool parts.
    ///
    /// # Errors
    ///
    /// Returns invalid gateway state if an active boundary sits beyond the accepted history.
    ///
    /// # Panics
    ///
    /// Panics if parser state slots disagree with their recorded boundary facts.
    pub(crate) fn routing_preparation(
        &self,
        parsed: &crate::format::ParsedRequest,
    ) -> Result<RoutingPreparation, CompactionError> {
        let mut projector = crate::affinity::fingerprint::Projector::new();
        let compaction = parsed.compaction.as_ref();
        let slots: Vec<_> = parsed
            .ir
            .state_items
            .iter()
            .enumerate()
            .filter_map(|(index, state)| {
                #[expect(
                    clippy::expect_used,
                    reason = "accepted codec state has a typed discriminator"
                )]
                let discriminator: StateDiscriminator =
                    StateDiscriminator::deserialize(&state.item)
                        .expect("accepted state has a type");
                matches!(discriminator, StateDiscriminator::Compaction).then_some(index)
            })
            .collect();
        let first_message = match self.active.as_ref() {
            Some(
                PreparedBoundary::Gateway { offset, .. } | PreparedBoundary::Native { offset, .. },
            ) => *offset,
            None => 0,
        };
        if first_message > parsed.ir.messages.len() {
            return Err(CompactionError::invalid_gateway_state(
                "the active checkpoint's boundary sits beyond the request's history",
            ));
        }
        let active_slot = self.active_index.map(|index| {
            #[expect(
                clippy::expect_used,
                reason = "the parser records each compaction state slot"
            )]
            *slots
                .get(index)
                .expect("the active boundary has a state slot")
        });
        let artifact = match self.active.as_ref() {
            Some(PreparedBoundary::Gateway { envelope, .. }) => {
                #[expect(
                    clippy::expect_used,
                    reason = "an active gateway boundary was authenticated from this request"
                )]
                let boundary = compaction
                    .and_then(|parsed| {
                        self.active_index
                            .and_then(|index| parsed.boundaries.get(index))
                    })
                    .expect("the authenticated gateway boundary exists");
                #[expect(
                    clippy::unreachable,
                    reason = "the retained preparation matches its original boundary"
                )]
                let CompactionBoundary::Gateway { carrier, .. } = boundary else {
                    unreachable!("the active gateway preparation has a gateway carrier");
                };
                let family = match parsed.affinity.client_format() {
                    UpstreamFormat::AnthropicMessages => {
                        crate::affinity::fingerprint::StateFamily::Messages
                    }
                    UpstreamFormat::OpenAiResponse => {
                        crate::affinity::fingerprint::StateFamily::Responses
                    }
                    #[expect(
                        clippy::unreachable,
                        reason = "only Responses and Messages parse gateway carriers"
                    )]
                    _ => unreachable!("gateway carriers belong to a state-bearing client codec"),
                };
                projector.gateway_state(family, carrier);
                Some(projector.gateway_artifact(&envelope.artifact_id))
            }
            Some(PreparedBoundary::Native { .. }) => {
                #[expect(
                    clippy::expect_used,
                    reason = "the active native boundary has a recorded state slot"
                )]
                let state = active_slot
                    .and_then(|index| parsed.ir.state_items.get(index))
                    .expect("native state exists");
                Some(projector.native_artifact(&state.item))
            }
            None => None,
        };
        let states = parsed
            .ir
            .state_items
            .iter()
            .enumerate()
            .filter_map(|(index, state)| {
                if active_slot.is_some_and(|active| index < active) || state.is_gateway {
                    return None;
                }
                if let Ok(boundary_index) = slots.binary_search(&index)
                    && compaction
                        .and_then(|parsed| parsed.boundaries.get(boundary_index))
                        .is_some_and(is_null_no_op)
                {
                    return None;
                }
                Some(state)
            });
        parsed
            .affinity
            .append_window(&parsed.ir.messages, states, first_message, &mut projector);
        Ok(RoutingPreparation {
            projector,
            artifact,
        })
    }

    /// Returns the authenticated producer requirement of the latest active native window.
    /// Portable summaries and unverified raw native artifacts impose no authenticated account.
    pub(crate) fn native_requirement(&self) -> Option<&NativeWindowPayload> {
        match self.active.as_ref()? {
            PreparedBoundary::Gateway { envelope, .. } => match &envelope.payload {
                EnvelopePayload::NativeWindow(payload) => Some(payload),
                EnvelopePayload::GatewaySummary(_) => None,
            },
            PreparedBoundary::Native { .. } => None,
        }
    }

    /// Authenticates every recognizable carrier in history order before choosing a target.
    /// A later usable checkpoint supersedes older state; native null blocks create no boundary.
    /// Messages visible content must exactly match its authenticated envelope binding.
    ///
    /// # Errors
    ///
    /// Returns a carrier authentication, unknown-key, or visible-content binding failure even
    /// when a later checkpoint would supersede the damaged carrier.
    pub(crate) fn authenticate(parsed: &ParsedCompaction) -> Result<Self, CompactionError> {
        let mut preparation = Self::default();
        for (index, boundary) in parsed.boundaries.iter().enumerate() {
            match boundary {
                CompactionBoundary::Gateway {
                    offset,
                    carrier,
                    visible_content,
                } => {
                    let binding = visible_content
                        .as_deref()
                        .map(|content| hex_sha256(content.as_bytes()));
                    let envelope = sealing::unseal_with_live_config(
                        carrier,
                        binding.as_deref().map(str::as_bytes),
                    )?;
                    match (&envelope.visible_content_digest, &binding) {
                        (Some(envelope_digest), Some(carrier_digest))
                            if envelope_digest == carrier_digest => {}
                        (None, None) => {}
                        _ => {
                            return Err(CompactionError::invalid_gateway_state(
                                "the gateway carrier's visible content does not match its binding",
                            ));
                        }
                    }
                    preparation.active_index = Some(index);
                    preparation.active = Some(PreparedBoundary::Gateway {
                        offset: *offset,
                        envelope,
                    });
                }
                CompactionBoundary::Native { offset, artifact } if !is_null_no_op(boundary) => {
                    preparation.active_index = Some(index);
                    preparation.active = Some(PreparedBoundary::Native {
                        offset: *offset,
                        artifact: *artifact,
                    });
                }
                CompactionBoundary::Native { .. } => {}
            }
        }
        Ok(preparation)
    }
}

/// Materializes the already authenticated active boundary using the existing replacement rules.
/// Superseded gateway carriers are removed without exporting their hidden content.
///
/// # Errors
///
/// Returns an invalid replacement boundary or target-incompatible native-window failure.
fn restore(
    ir: &mut IrRequest,
    parsed: &ParsedCompaction,
    preparation: &BoundaryPreparation,
    target_format: UpstreamFormat,
) -> Result<Option<RestoredBoundary>, CompactionError> {
    let Some(active) = &preparation.active else {
        remove_gateway_state_items(ir);
        return Ok(None);
    };
    match active {
        PreparedBoundary::Gateway { offset, envelope } => {
            match &envelope.payload {
                EnvelopePayload::GatewaySummary(payload) => {
                    restore_gateway_summary(ir, parsed, *offset, payload)?;
                }
                EnvelopePayload::NativeWindow(payload) => {
                    restore_native_window(ir, *offset, payload, target_format)?;
                }
            }
            Ok(Some(RestoredBoundary::Gateway {
                envelope: envelope.clone(),
            }))
        }
        PreparedBoundary::Native { artifact, .. } => {
            remove_gateway_state_items(ir);
            Ok(Some(RestoredBoundary::Native(*artifact)))
        }
    }
}

/// Composes target-independent authentication and restoration for domain-level test fixtures.
///
/// # Errors
///
/// Returns a carrier, boundary, or native-format compatibility failure.
#[cfg(test)]
pub(crate) fn prepare(
    ir: &mut IrRequest,
    parsed: &ParsedCompaction,
    target_format: UpstreamFormat,
) -> Result<Option<RestoredBoundary>, CompactionError> {
    let preparation = BoundaryPreparation::authenticate(parsed)?;
    restore(ir, parsed, &preparation, target_format)
}

/// Prepares authenticated state against the selected producer, permitting only explicit
/// readable-summary export. `preparation` must belong to this same parsed request.
///
/// # Errors
///
/// Returns a producer, model, format, retained-state, or replacement-boundary incompatibility.
pub(crate) fn prepare_for_target(
    ir: &mut IrRequest,
    parsed: &ParsedCompaction,
    routing: &crate::provider::ModelRouting,
    preparation: &BoundaryPreparation,
) -> Result<Option<RestoredBoundary>, CompactionError> {
    let target_format = routing.provider.definition.payload.format;
    let can_export = routing
        .provider
        .definition
        .payload
        .compaction
        .as_ref()
        .is_some_and(|section| section.can_export_native_summary);
    let mut exported_retained = Vec::new();
    if let Some(active) = &preparation.active {
        let mut exported = None;
        match active {
            PreparedBoundary::Gateway { envelope, .. } => {
                if let EnvelopePayload::NativeWindow(payload) = &envelope.payload {
                    let is_compatible = payload.producer_instance_id
                        == format!("provider:{}", routing.provider.id)
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
            PreparedBoundary::Native {
                offset,
                artifact: NativeArtifact::MessagesCompactionBlock { .. },
            } if target_format != UpstreamFormat::AnthropicMessages && can_export => {
                exported = ir
                    .state_items
                    .iter()
                    .find(|item| item.offset == *offset && !item.is_gateway)
                    .and_then(|item| readable_native_summary(&item.item));
            }
            _ => {}
        }
        if let Some(summary) = exported {
            let offset = match active {
                PreparedBoundary::Gateway { offset, .. }
                | PreparedBoundary::Native { offset, .. } => *offset,
            };
            if ir.messages.iter().skip(offset).any(|message| {
                message
                    .parts
                    .iter()
                    .any(|part| matches!(part, crate::ir::IrPart::Reasoning { .. }))
            }) || ir
                .state_items
                .iter()
                .any(|item| item.offset > offset && !item.is_gateway)
            {
                return Err(CompactionError::inaccessible_native_state(
                    "readable export cannot preserve opaque retained state",
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
            ir.messages = vec![summary_message(&summary)];
            ir.messages.extend(exported_retained);
            ir.messages.extend(fresh);
            ir.state_items.clear();
            return Ok(None);
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
fn restore_gateway_summary(
    ir: &mut IrRequest,
    parsed: &ParsedCompaction,
    offset: usize,
    payload: &GatewaySummaryPayload,
) -> Result<(), CompactionError> {
    if offset > ir.messages.len() {
        return Err(CompactionError::invalid_gateway_state(
            "the gateway checkpoint's boundary sits beyond the request's history",
        ));
    }

    let fresh: Vec<IrMessage> = ir.messages.split_off(offset);

    // The identity pass: a current instruction item proven to overlap the
    // checkpoint's covered identities is superseded; an unmatched one — a new
    // developer instruction, for instance — is preserved.
    let mut preserved_current: Vec<&crate::format::ParsedInstructionItem> = Vec::new();

    let mut occurrences: Vec<(InstructionRole, usize)> = Vec::new();

    for item in &parsed.instruction_items {
        let occurrence = occurrences
            .iter()
            .find(|(role, _)| *role == item.role)
            .map(|(_, count)| *count)
            .unwrap_or(0);

        if let Some((_, count)) = occurrences.iter_mut().find(|(role, _)| *role == item.role) {
            *count += 1;
        } else {
            occurrences.push((item.role, 1));
        }

        let identity = ItemIdentity::for_instruction(item.role, occurrence, &item.text, None);

        if !payload
            .overlap_identities
            .iter()
            .any(|covered| covered.matches(&identity))
        {
            preserved_current.push(item);
        }
    }

    // Rebuild the instruction context: the checkpoint's preserved instructions
    // first, then the current instructions that survived the overlap pass.
    let mut instruction_parts: Vec<String> = payload
        .preserved_instructions
        .iter()
        .map(|instruction| instruction.text.clone())
        .collect();

    instruction_parts.extend(preserved_current.iter().map(|item| item.text.clone()));

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
    // summary message plus the retained messages now precede it.
    let fresh_state_items: Vec<StateItem> = ir
        .state_items
        .iter()
        .filter(|item| !item.is_gateway && item.offset >= offset)
        .map(|item| StateItem {
            offset: 1 + retained_message_count + (item.offset - offset),
            item: item.item.clone(),
            is_gateway: false,
        })
        .collect();

    ir.state_items = payload.retained_state_items.clone();
    ir.state_items.extend(fresh_state_items);

    Ok(())
}

/// Restores a wrapped native window: drops the covered prefix and places the
/// exact native artifact at the boundary — a whole Responses window replayed
/// as state items, or an Anthropic on-demand block at the leading placement
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
pub(crate) fn produced_prefix_identity(ir: &IrRequest) -> PrefixIdentity {
    let instructions_digest = ir
        .instructions
        .as_deref()
        .map(|instructions| hex_digest(instructions.as_bytes()));

    let tools_digest = if ir.tools.is_empty() {
        None
    } else {
        #[expect(
            clippy::expect_used,
            reason = "serializing concrete IR tools cannot fail"
        )]
        let canonical = serde_json::to_vec(&ir.tools).expect("the tool definitions serialize");

        Some(hex_digest(&canonical))
    };

    PrefixIdentity {
        instructions_digest,
        tools_digest,
    }
}

/// Renders a SHA-256 digest as lowercase hex.
fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();

    hasher.update(bytes);

    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut rendered, byte| {
            rendered.push_str(&format!("{byte:02x}"));

            rendered
        })
}

/// its replay contract demands (cross-protocol continuation never applies the
/// source protocol's historical placement to the target payload).
fn restore_native_window(
    ir: &mut IrRequest,
    offset: usize,
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
    // state of the fresh suffix rides along at its recomputed position.
    let fresh_state_items: Vec<StateItem> = ir
        .state_items
        .iter()
        .filter(|item| !item.is_gateway && item.offset >= offset)
        .map(|item| StateItem {
            offset: payload.retained_messages.len() + item.offset - offset,
            item: item.item.clone(),
            is_gateway: false,
        })
        .collect();

    let mut restored_state_items = Vec::new();

    match &payload.window {
        NativeReplayWindow::ResponsesWindow { items } => {
            // The whole window replays as one block at the boundary: the render
            // splices same-offset state items in their vector order.
            restored_state_items.extend(items.iter().map(|item| StateItem {
                offset: 0,
                item: item.clone(),
                is_gateway: false,
            }));
        }

        NativeReplayWindow::AnthropicOnDemandBlock { block } => {
            // Leading placement: the block opens the restored window and the
            // fresh suffix follows once.
            restored_state_items.push(StateItem {
                offset: 0,
                item: block.clone(),
                is_gateway: false,
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
pub(crate) fn native_continuation(
    ir: &IrRequest,
    payload: &NativeWindowPayload,
) -> Result<IrRequest, CompactionError> {
    let mut continuation = ir.clone();
    let boundary = continuation.messages.len();
    restore_native_window(&mut continuation, boundary, payload, payload.native_format)?;
    Ok(continuation)
}

/// The gateway checkpoint plan (design §16.3): the eligible older prefix the
/// summary will cover, the retained interaction units and the native state
/// riding them, and the preserved instruction context with its coverage
/// identities.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GatewayCheckpointPlan {
    /// The eligible older prefix the summary covers, as input history.
    pub(crate) summary_input_messages: Vec<IrMessage>,

    /// The retained interaction units with their identities, in conversation
    /// order. Each unit is indivisible: whole tool exchanges stay together.
    pub(crate) retained_units: Vec<(ItemIdentity, Vec<IrMessage>)>,

    /// The native state items positioned inside the retained region, with
    /// offsets recomputed against the restored window layout (one summary
    /// message, then the retained units' messages).
    pub(crate) retained_state_items: Vec<StateItem>,

    /// The current instruction items with their identities and roles.
    pub(crate) preserved_instructions: Vec<(ItemIdentity, InstructionRole, String)>,

    /// The identities the checkpoint covers: the preserved instruction items
    /// and the retained units.
    pub(crate) overlap_identities: Vec<ItemIdentity>,

    /// The summary output budget in tokens.
    pub(crate) summary_budget: u64,

    /// The retained-input soft target in tokens.
    pub(crate) retention_budget: u64,
}

/// One interaction unit: an indivisible run of messages covering a turn and its
/// whole tool exchange.
struct InteractionUnit {
    /// The unit's identity: the identity of its first message.
    identity: ItemIdentity,

    /// The unit's messages.
    messages: Vec<IrMessage>,

    /// The message index the unit starts at.
    start: usize,
}

/// Plans a gateway checkpoint from the restored window (design §16.3): groups
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
pub(crate) fn plan_gateway_checkpoint(
    ir: &IrRequest,
    instruction_items: &[crate::format::ParsedInstructionItem],
    mandatory_active_turn: bool,
    budgets: (u64, u64),
) -> Result<GatewayCheckpointPlan, CompactionError> {
    plan_checkpoint(ir, instruction_items, mandatory_active_turn, budgets, false)
}

/// Plans a native checkpoint without rejecting opaque state the native producer can consume.
pub(crate) fn plan_native_checkpoint(
    ir: &IrRequest,
    instruction_items: &[crate::format::ParsedInstructionItem],
    budgets: (u64, u64),
) -> Result<GatewayCheckpointPlan, CompactionError> {
    plan_checkpoint(ir, instruction_items, true, budgets, true)
}

/// Selects whole interaction units for a contiguous retained suffix.
fn plan_checkpoint(
    ir: &IrRequest,
    instruction_items: &[crate::format::ParsedInstructionItem],
    mandatory_active_turn: bool,
    budgets: (u64, u64),
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

    // Images have no text estimate. Retain the entire suffix starting at the earliest
    // image rather than discarding modality data or assigning it a fabricated size.
    if let Some(index) = units.iter().position(|unit| {
        unit.messages.iter().any(|message| {
            message
                .parts
                .iter()
                .any(|part| matches!(part, crate::ir::IrPart::Image { .. }))
        })
    }) {
        mandatory_start_index = mandatory_start_index.min(index);
    }

    // Retention: recent complete units backward under the budget; mandatory
    // units always included — the budget is a soft target for mandatory state.
    let mut retained: Vec<&InteractionUnit> = Vec::new();

    let mut cumulative_tokens = 0u64;

    for (unit_index, unit) in units.iter().enumerate().rev() {
        let is_multimodal = unit.messages.iter().any(|message| {
            message
                .parts
                .iter()
                .any(|part| matches!(part, crate::ir::IrPart::Image { .. }))
        });
        let unit_tokens = if is_multimodal {
            0
        } else {
            super::threshold::estimate_message_tokens(&unit.messages)?
        };

        let is_mandatory = unit_index >= mandatory_start_index || is_multimodal;

        if is_mandatory || cumulative_tokens + unit_tokens <= budgets.1 {
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
        })
        .collect();

    // The current instruction items with their identities.
    let materialized_instructions;
    let instruction_items = if ir.instructions.as_deref()
        != Some(
            instruction_items
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n")
                .as_str(),
        )
        && ir.instructions.is_some()
    {
        materialized_instructions = vec![crate::format::ParsedInstructionItem {
            role: InstructionRole::System,
            text: ir.instructions.clone().unwrap_or_default(),
        }];
        materialized_instructions.as_slice()
    } else {
        instruction_items
    };
    let mut role_occurrences: Vec<(InstructionRole, usize)> = Vec::new();

    let mut preserved_instructions = Vec::with_capacity(instruction_items.len());

    for item in instruction_items {
        let occurrence = role_occurrences
            .iter()
            .find(|(role, _)| *role == item.role)
            .map(|(_, count)| *count)
            .unwrap_or(0);

        if let Some((_, count)) = role_occurrences
            .iter_mut()
            .find(|(role, _)| *role == item.role)
        {
            *count += 1;
        } else {
            role_occurrences.push((item.role, 1));
        }

        preserved_instructions.push((
            ItemIdentity::for_instruction(item.role, occurrence, &item.text, None),
            item.role,
            item.text.clone(),
        ));
    }

    let retained_units: Vec<(ItemIdentity, Vec<IrMessage>)> = retained
        .iter()
        .map(|unit| (unit.identity.clone(), unit.messages.clone()))
        .collect();

    let mut overlap_identities: Vec<ItemIdentity> = preserved_instructions
        .iter()
        .map(|(identity, _role, _text)| identity.clone())
        .collect();

    overlap_identities.extend(retained_units.iter().map(|(identity, _)| identity.clone()));

    Ok(GatewayCheckpointPlan {
        summary_input_messages: older_prefix
            .iter()
            .flat_map(|unit| unit.messages.iter().cloned())
            .collect(),
        retained_units,
        retained_state_items,
        preserved_instructions,
        overlap_identities,
        summary_budget: budgets.0,
        retention_budget: budgets.1,
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
                identity: ItemIdentity::for_message(message.role, index, message, None),
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
pub(crate) fn seal_envelope(
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

/// Builds and seals the gateway summary checkpoint of one plan (design §16.3).
///
/// # Errors
///
/// Returns [`CompactionError`] as [`seal_envelope`].
pub(crate) fn seal_gateway_checkpoint(
    plan: &GatewayCheckpointPlan,
    summary_text: String,
    replacement: ReplacementMode,
    visible_content: Option<&str>,
) -> Result<(EnvelopeV1, String), CompactionError> {
    let payload = EnvelopePayload::GatewaySummary(GatewaySummaryPayload {
        summary_text,
        preserved_instructions: plan
            .preserved_instructions
            .iter()
            .map(
                |(identity, role, text)| super::envelope::PreservedInstruction {
                    identity: identity.clone(),
                    role: *role,
                    text: text.clone(),
                },
            )
            .collect(),
        retained_units: plan
            .retained_units
            .iter()
            .map(|(identity, messages)| super::envelope::RetainedUnit {
                identity: identity.clone(),
                messages: messages.clone(),
            })
            .collect(),
        retained_state_items: plan.retained_state_items.clone(),
        overlap_identities: plan.overlap_identities.clone(),
    });

    seal_envelope(payload, replacement, visible_content)
}

/// Builds and seals a wrapped native window checkpoint (design §6.3): the exact
/// native artifact beside the provenance needed to replay it against its
/// producer.
///
/// # Errors
///
/// Returns [`CompactionError`] as [`seal_envelope`].
pub(crate) fn seal_native_window(
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

#[cfg(test)]
mod tests {
    #![expect(
        clippy::expect_used,
        reason = "test fixtures seal and unseal values constructed to succeed"
    )]
    #![expect(
        clippy::indexing_slicing,
        reason = "fixtures index values they constructed themselves"
    )]

    use super::{
        GatewayCheckpointPlan, RestoredBoundary, hex_sha256, interaction_units,
        plan_gateway_checkpoint, prepare, seal_gateway_checkpoint,
    };
    use crate::{
        compaction::{
            envelope::{EnvelopePayload, ReplacementMode},
            error::CompactionError,
            identity::InstructionRole,
        },
        format::{CompactionControl, NativeArtifact, ParsedCompaction, UpstreamFormat},
        ir::{IrMessage, IrPart, IrRequest, IrRole, StateItem},
    };
    use serde_json::json;

    /// Builds a text message.
    fn text(role: IrRole, text: &str) -> IrMessage {
        IrMessage {
            role,
            parts: vec![IrPart::Text {
                text: String::from(text),
            }],
        }
    }

    /// The interaction units group whole tool exchanges and reject an orphan
    /// tool result.
    ///
    /// # Panics
    ///
    /// Panics if the unit grouping diverges from the indivisible-unit rule.
    #[test]
    fn interaction_units_group_whole_exchanges() {
        let messages = vec![
            text(IrRole::User, "u1"),
            text(IrRole::Assistant, "a1"),
            text(IrRole::Tool, "t1"),
            text(IrRole::Assistant, "a2"),
            text(IrRole::User, "u2"),
        ];

        let units = interaction_units(&messages).expect("the history is well-formed");

        // Units: [u1], [a1, t1, a2] (the assistant after the user opens a unit;
        // the tool result joins it; the assistant after the tool extends it),
        // and [u2].
        assert_eq!(units.len(), 3);
        assert_eq!(units[0].messages.len(), 1);
        assert_eq!(units[1].messages.len(), 3);
        assert_eq!(units[2].messages.len(), 1);
        assert_eq!(units[0].start, 0);
        assert_eq!(units[1].start, 1);
        assert_eq!(units[2].start, 4);

        let orphan = vec![text(IrRole::Tool, "t1")];

        assert!(matches!(
            interaction_units(&orphan),
            Err(CompactionError::UnusableHistory { .. })
        ));
    }

    /// The planner retains recent units under the budget, keeps the mandatory
    /// active turn, and fails when no older prefix remains.
    ///
    /// # Panics
    ///
    /// Panics if the plan diverges from the retention rules.
    #[test]
    fn plans_retention_and_mandatory_state() {
        let ir = IrRequest {
            has_compaction_trigger: false,
            model: String::new(),
            instructions: None,
            messages: vec![
                text(IrRole::User, "old question"),
                text(IrRole::Assistant, "old answer"),
                text(IrRole::User, "recent question"),
                text(IrRole::Assistant, "recent answer"),
                text(IrRole::User, "pending turn"),
            ],
            tools: Vec::new(),
            tool_choice: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            response_format: None,
            reasoning_effort: None,
            state_items: Vec::new(),
        };

        let instruction_items = Vec::new();

        // Each fixture message is one unit; the mandatory active turn (the
        // pending user message's unit) is always retained, and a budget of one
        // token retains nothing beyond it.
        let plan = plan_gateway_checkpoint(&ir, &instruction_items, true, (4096, 1))
            .expect("the plan retains the mandatory turn");

        assert_eq!(plan.summary_input_messages.len(), 4);
        assert_eq!(plan.retained_units.len(), 1);

        // Compact-only: a zero retention budget retains nothing, so the whole
        // history is the summarized prefix — a legitimate compaction, since a
        // final user turn may be summarized.
        let plan = plan_gateway_checkpoint(&ir, &instruction_items, false, (4096, 0))
            .expect("a compact-only zero budget summarizes the whole history");

        assert_eq!(plan.summary_input_messages.len(), 5);
        assert_eq!(plan.retained_units.len(), 0);

        // A generous budget retains everything, leaving no older prefix.
        let plan = plan_gateway_checkpoint(&ir, &instruction_items, false, (4096, 100_000))
            .expect_err("retaining everything leaves nothing to summarize");

        assert!(matches!(plan, CompactionError::BudgetExceeded { .. }));

        // A single-turn compact-and-continue request: the mandatory active
        // turn covers the whole history, and no older prefix remains.
        let single_turn = IrRequest {
            has_compaction_trigger: false,
            model: String::new(),
            instructions: None,
            messages: vec![text(IrRole::User, "only turn")],
            tools: Vec::new(),
            tool_choice: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            response_format: None,
            reasoning_effort: None,
            state_items: Vec::new(),
        };

        let plan = plan_gateway_checkpoint(&single_turn, &instruction_items, true, (4096, 8192));

        assert!(matches!(plan, Err(CompactionError::BudgetExceeded { .. })));
    }

    /// Native state inside the summarized prefix fails the plan; instruction
    /// identities ride the coverage set.
    ///
    /// # Panics
    ///
    /// Panics if the plan diverges from the state and identity rules.
    #[test]
    fn plans_state_and_instruction_coverage() {
        let ir = IrRequest {
            has_compaction_trigger: false,
            model: String::new(),
            instructions: None,
            messages: vec![
                text(IrRole::User, "old question"),
                text(IrRole::Assistant, "old answer"),
                text(IrRole::User, "pending turn"),
            ],
            tools: Vec::new(),
            tool_choice: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            response_format: None,
            reasoning_effort: None,
            state_items: vec![StateItem {
                offset: 2,
                item: json!({"type": "reasoning"}),
                is_gateway: false,
            }],
        };

        let instruction_items = vec![crate::format::ParsedInstructionItem {
            role: InstructionRole::System,
            text: String::from("be terse"),
        }];

        // Compact-and-continue: the mandatory active turn is the pending user
        // message's unit alone, and the state item at its head rides it at the
        // recomputed restored-window offset (one summary message + zero
        // retained messages before it).
        let plan = plan_gateway_checkpoint(&ir, &instruction_items, true, (4096, 0))
            .expect("the plan retains the mandatory turn");

        assert_eq!(plan.summary_input_messages.len(), 2);
        assert_eq!(plan.retained_units.len(), 1);
        assert!(
            matches!(
                plan.retained_state_items.as_slice(),
                [StateItem { offset: 1, .. }]
            ),
            "the state item rides the retained region at its recomputed offset"
        );

        assert_eq!(plan.preserved_instructions.len(), 1);
        assert_eq!(plan.overlap_identities.len(), 2);

        // A compact-only budget of zero summarizes the whole history, whose
        // native state cannot be summarized: an explicit failure.
        let plan = plan_gateway_checkpoint(&ir, &instruction_items, false, (4096, 0));

        assert!(matches!(
            plan,
            Err(CompactionError::InaccessibleNativeState { .. })
        ));
    }

    /// Builds an empty prepared-request fixture pair.
    fn fixture_ir(messages: Vec<IrMessage>, state_items: Vec<StateItem>) -> IrRequest {
        IrRequest {
            has_compaction_trigger: false,
            model: String::new(),
            instructions: Some(String::from("be terse")),
            messages,
            tools: Vec::new(),
            tool_choice: None,
            temperature: None,
            top_p: None,
            max_output_tokens: None,
            response_format: None,
            reasoning_effort: None,
            state_items,
        }
    }

    /// A gateway summary envelope restores its window: the covered prefix is
    /// superseded, the summary and retained units are restored once, the fresh
    /// suffix and its native state ride at recomputed offsets, and unmatched
    /// current instructions survive while covered ones are dropped.
    ///
    /// # Panics
    ///
    /// Panics if the restored window diverges from the boundary semantics.
    #[test]
    fn prepare_restores_a_gateway_summary_checkpoint() {
        let _configuration_guard = crate::test_support::configuration_lock();
        let _configuration = crate::test_support::install_global_configuration();

        // The checkpoint covers messages 0..2 and retains unit [1..2].
        let checkpoint_messages = [text(IrRole::User, "old"), text(IrRole::User, "kept")];

        let plan = GatewayCheckpointPlan {
            summary_input_messages: Vec::new(),
            retained_units: vec![(
                super::ItemIdentity::for_message(IrRole::User, 0, &checkpoint_messages[1], None),
                vec![checkpoint_messages[1].clone()],
            )],
            retained_state_items: Vec::new(),
            preserved_instructions: vec![(
                super::ItemIdentity::for_instruction(
                    InstructionRole::System,
                    0,
                    "old instruction",
                    None,
                ),
                InstructionRole::System,
                String::from("old instruction"),
            )],
            overlap_identities: vec![super::ItemIdentity::for_instruction(
                InstructionRole::System,
                0,
                "old instruction",
                None,
            )],
            summary_budget: 4096,
            retention_budget: 8192,
        };

        let (_envelope, carrier) = seal_gateway_checkpoint(
            &plan,
            String::from("the summary"),
            ReplacementMode::HistoricalCheckpoint,
            None,
        )
        .expect("the checkpoint seals");

        // The replayed request: the same old instruction, a covered old user
        // message at the boundary offset, the checkpoint carrier, and fresh
        // history after it.
        let mut ir = fixture_ir(
            vec![text(IrRole::User, "old"), text(IrRole::User, "new turn")],
            vec![StateItem {
                offset: 1,
                item: json!({"type": "reasoning"}),
                is_gateway: false,
            }],
        );

        ir.state_items.push(StateItem {
            offset: 1,
            item: json!({"type": "compaction"}),
            is_gateway: true,
        });

        let parsed = ParsedCompaction {
            control: None,
            boundaries: vec![crate::format::CompactionBoundary::Gateway {
                offset: 1,
                carrier,
                visible_content: None,
            }],
            instruction_items: vec![crate::format::ParsedInstructionItem {
                role: InstructionRole::System,
                text: String::from("old instruction"),
            }],
        };

        let boundary = prepare(&mut ir, &parsed, UpstreamFormat::OpenAiResponse)
            .expect("the checkpoint restores");

        assert!(matches!(boundary, Some(RestoredBoundary::Gateway { .. })));

        // The restored window: summary message, retained unit, fresh turn.
        assert_eq!(ir.messages.len(), 3);
        assert_eq!(ir.messages[0].role, IrRole::User);
        assert_eq!(ir.messages[1], checkpoint_messages[1]);
        assert_eq!(ir.messages[2], text(IrRole::User, "new turn"));

        // The covered instruction is superseded by identity overlap; the
        // preserved instruction rides alone.
        assert_eq!(ir.instructions.as_deref(), Some("old instruction"));

        // No gateway item survives; the fresh suffix's native state rides at
        // its recomputed offset (1 summary + 1 retained message = 2).
        assert_eq!(ir.state_items.len(), 1);
        assert!(!ir.state_items[0].is_gateway);
        assert_eq!(ir.state_items[0].offset, 2);
    }

    /// A wrapped native window restores against its own format and rejects a
    /// foreign one; the fresh suffix stays.
    ///
    /// # Panics
    ///
    /// Panics if the restored window diverges from the wrapping rules.
    #[test]
    fn prepare_restores_a_wrapped_native_window() {
        let _configuration_guard = crate::test_support::configuration_lock();
        let _configuration = crate::test_support::install_global_configuration();

        let window = super::NativeReplayWindow::ResponsesWindow {
            items: vec![json!({"type": "compaction"})],
        };

        let (_envelope, carrier) = super::seal_envelope(
            EnvelopePayload::NativeWindow(super::NativeWindowPayload {
                retained_messages: Vec::new(),
                retained_state_items: Vec::new(),
                producer_instance_id: String::from("provider:1"),
                upstream_model_id: String::from("m"),
                mechanism: crate::compaction::mechanism::Mechanism::MessagesOnDemandCompaction,
                native_format: UpstreamFormat::AnthropicMessages,
                required_prefix_identity: None,
                window,
            }),
            ReplacementMode::CanonicalWindow,
            Some("bound content"),
        )
        .expect("the window seals");

        let mut ir = fixture_ir(
            vec![text(IrRole::User, "covered"), text(IrRole::User, "fresh")],
            Vec::new(),
        );

        let parsed = ParsedCompaction {
            control: Some(CompactionControl::ResponsesTrigger),
            boundaries: vec![crate::format::CompactionBoundary::Gateway {
                offset: 1,
                carrier,
                visible_content: Some(String::from("bound content")),
            }],
            instruction_items: Vec::new(),
        };

        // The same format: the window restores.
        let boundary = prepare(&mut ir, &parsed, UpstreamFormat::AnthropicMessages)
            .expect("the wrapped window restores");

        assert!(matches!(boundary, Some(RestoredBoundary::Gateway { .. })));
        assert_eq!(ir.messages, vec![text(IrRole::User, "fresh")]);
        assert_eq!(ir.state_items.len(), 1);
        assert_eq!(ir.state_items[0].offset, 0);

        // A foreign target format rejects the window: wrapping does not enable
        // cross-vendor native conversion.
        let mut ir = fixture_ir(
            vec![text(IrRole::User, "covered"), text(IrRole::User, "fresh")],
            Vec::new(),
        );

        let parsed = ParsedCompaction {
            control: None,
            boundaries: vec![crate::format::CompactionBoundary::Gateway {
                offset: 1,
                carrier: {
                    let (_envelope, carrier) = super::seal_envelope(
                        EnvelopePayload::NativeWindow(super::NativeWindowPayload {
                            retained_messages: Vec::new(),
                            retained_state_items: Vec::new(),
                            producer_instance_id: String::from("provider:1"),
                            upstream_model_id: String::from("m"),
                            mechanism:
                                crate::compaction::mechanism::Mechanism::MessagesOnDemandCompaction,
                            native_format: UpstreamFormat::AnthropicMessages,
                            required_prefix_identity: None,
                            window: super::NativeReplayWindow::ResponsesWindow {
                                items: vec![json!({"type": "compaction"})],
                            },
                        }),
                        ReplacementMode::CanonicalWindow,
                        None,
                    )
                    .expect("the window seals");

                    carrier
                },
                visible_content: None,
            }],
            instruction_items: Vec::new(),
        };

        let restored = prepare(&mut ir, &parsed, UpstreamFormat::OpenAiResponse);

        assert!(matches!(
            restored,
            Err(CompactionError::InaccessibleNativeState { .. })
        ));
    }

    /// A null/no-op native block establishes no boundary, and native
    /// passthrough leaves the history untouched while gateway items are
    /// removed.
    ///
    /// # Panics
    ///
    /// Panics if the boundary semantics diverge.
    #[tokio::test]
    async fn prepare_rejects_a_prefix_mismatched_native_window() {
        let _configuration_guard = crate::test_support::configuration_lock();
        let _configuration = crate::test_support::install_global_configuration();

        // The wrap records the producer's prefix: the instructions the window
        // was produced under.
        let mut ir = fixture_ir(
            vec![text(IrRole::User, "covered"), text(IrRole::User, "fresh")],
            Vec::new(),
        );

        ir.instructions = Some(String::from("produce under this"));

        let prefix = super::produced_prefix_identity(&ir);

        ir.instructions = None;

        let (_envelope, carrier) = super::seal_envelope(
            EnvelopePayload::NativeWindow(super::NativeWindowPayload {
                retained_messages: Vec::new(),
                retained_state_items: Vec::new(),
                producer_instance_id: String::from("provider:1"),
                upstream_model_id: String::from("m"),
                mechanism: crate::compaction::mechanism::Mechanism::MessagesOnDemandCompaction,
                native_format: UpstreamFormat::AnthropicMessages,
                required_prefix_identity: Some(prefix),
                window: super::NativeReplayWindow::AnthropicOnDemandBlock {
                    block: json!({
                        "type": "compaction",
                        "content": "s",
                        "signature": "sig",
                    }),
                },
            }),
            ReplacementMode::CanonicalWindow,
            None,
        )
        .expect("the window seals");

        // Replaying under the same prefix restores; replaying under a changed
        // one fails explicitly instead of handing the producer state it must
        // reject.
        let mut same = fixture_ir(
            vec![text(IrRole::User, "covered"), text(IrRole::User, "fresh")],
            Vec::new(),
        );

        same.instructions = Some(String::from("produce under this"));

        let parsed = ParsedCompaction {
            control: None,
            boundaries: vec![crate::format::CompactionBoundary::Gateway {
                offset: 1,
                carrier: carrier.clone(),
                visible_content: None,
            }],
            instruction_items: Vec::new(),
        };

        let boundary = prepare(&mut same, &parsed, UpstreamFormat::AnthropicMessages)
            .expect("the matching prefix restores");

        assert!(matches!(boundary, Some(RestoredBoundary::Gateway { .. })));

        let mut changed = fixture_ir(
            vec![text(IrRole::User, "covered"), text(IrRole::User, "fresh")],
            Vec::new(),
        );

        changed.instructions = Some(String::from("changed"));

        let parsed = ParsedCompaction {
            control: None,
            boundaries: vec![crate::format::CompactionBoundary::Gateway {
                offset: 1,
                carrier,
                visible_content: None,
            }],
            instruction_items: Vec::new(),
        };

        assert!(
            prepare(&mut changed, &parsed, UpstreamFormat::AnthropicMessages).is_err(),
            "a changed prefix must fail the restore explicitly"
        );
    }

    #[test]
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    fn prepare_treats_a_null_no_op_block_as_no_boundary() {
        let _configuration_guard = crate::test_support::configuration_lock();
        let _configuration = crate::test_support::install_global_configuration();

        let mut ir = fixture_ir(
            vec![text(IrRole::User, "kept")],
            vec![
                StateItem {
                    offset: 0,
                    item: json!({"type": "compaction", "content": null}),
                    is_gateway: false,
                },
                StateItem {
                    offset: 0,
                    item: json!({"type": "compaction"}),
                    is_gateway: true,
                },
            ],
        );

        let parsed = ParsedCompaction {
            control: None,
            boundaries: vec![
                crate::format::CompactionBoundary::Native {
                    offset: 0,
                    artifact: NativeArtifact::MessagesCompactionBlock {
                        is_null_no_op: true,
                        placement: crate::format::BoundaryPlacement::Leading,
                    },
                },
                crate::format::CompactionBoundary::Gateway {
                    offset: 0,
                    carrier: String::from("not-a-carrier-but-decoded-above? no: never reached"),
                    visible_content: None,
                },
            ],
            instruction_items: Vec::new(),
        };

        // The null no-op block is the last non-no-op boundary candidate after
        // skipping — but the trailing gateway boundary decodes first and a
        // non-marker carrier fails explicitly.
        let restored = prepare(&mut ir, &parsed, UpstreamFormat::AnthropicMessages);

        assert!(
            matches!(restored, Err(CompactionError::InvalidGatewayState { .. })),
            "a damaged gateway marker fails explicitly even when superseded"
        );

        // Without the damaged gateway boundary, the null no-op block yields no
        // boundary, the history stands, and no gateway item survives.
        let mut ir = fixture_ir(
            vec![text(IrRole::User, "kept")],
            vec![StateItem {
                offset: 0,
                item: json!({"type": "compaction", "content": null}),
                is_gateway: false,
            }],
        );

        let parsed = ParsedCompaction {
            control: None,
            boundaries: vec![crate::format::CompactionBoundary::Native {
                offset: 0,
                artifact: NativeArtifact::MessagesCompactionBlock {
                    is_null_no_op: true,
                    placement: crate::format::BoundaryPlacement::Leading,
                },
            }],
            instruction_items: Vec::new(),
        };

        // The null block establishes no boundary, and no earlier boundary
        // exists: no boundary stands, the history stands, and the native state
        // is untouched.
        let boundary = prepare(&mut ir, &parsed, UpstreamFormat::AnthropicMessages)
            .expect("the null block establishes no boundary");

        assert_eq!(boundary, None);
        assert_eq!(ir.messages, vec![text(IrRole::User, "kept")]);
        assert_eq!(ir.state_items.len(), 1);
    }

    /// The identity digest helper renders the SHA-256 of its bytes as hex.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn hex_sha256_is_the_documented_digest_form() {
        // SHA-256 of the empty string.
        assert_eq!(
            hex_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
