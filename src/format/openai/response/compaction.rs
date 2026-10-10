//! Remote-compaction wire behavior of the Responses codec: the standalone
//! compact endpoint's request and response payloads, the compaction state item
//! builders shared by parsing and rendering, and the trigger lifecycle's
//! streaming renderer.
//!
//! The compact endpoint (`docs/design/architecture.md`) is not a generation request at a
//! different URL: its body carries no generation-only controls and no `stream`
//! member, its upstream framing is always non-streaming, and its success body
//! is a `response.compaction` object whose output is the canonical next
//! window.

use super::{
    client_render::{ResponseUsage, SseRenderer},
    request_parse::{ItemIn, OptionalReplayText, build_instructions, convert_input},
    upstream_parse::CompactedResponseNative,
};
use crate::{
    format::{
        self, ClientSseRenderer as _, CompactionControl, ParsedCompaction, ParsedRequest,
        StringOrArray,
    },
    ids,
    ir::{InvalidRequest, IrEvent, IrRequest, IrUsage},
    unix_time,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The closed object discriminator of a standalone compaction response.
#[derive(Debug, Clone, Serialize)]
enum CompactedResponseObject {
    /// Canonical client-held next window.
    #[serde(rename = "response.compaction")]
    Compaction,
}

/// The strictly parsed standalone compact request body.
///
/// The endpoint carries no generation-only controls — no tools, sampling,
/// reasoning, text, or streaming members — so a body carrying one is rejected
/// by the strict shape itself. The caching and tiering conventions are the
/// ordinary Responses envelope's accepted-and-ignored controls (`docs/reference/configuration.md`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompactRequestBody {
    /// Public model name to route to.
    model: String,

    /// Conversation input: a plain string or an ordered item array.
    input: StringOrArray<ItemIn>,

    /// System instructions.
    #[serde(default)]
    instructions: Option<String>,

    /// Stateful continuation identifier; the gateway is stateless, so a
    /// present, non-null value is rejected rather than silently dropped.
    #[serde(default)]
    previous_response_id: Option<String>,

    /// Upstream prompt-cache placement key; accepted and ignored (`docs/reference/inference.md`).
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
    prompt_cache_key: Option<String>,

    /// Upstream prompt-cache options; accepted and ignored (`docs/reference/inference.md`).
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
    prompt_cache_options: Option<PromptCacheOptions>,

    /// Upstream prompt-cache retention hint; accepted and ignored (`docs/reference/inference.md`).
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
    prompt_cache_retention: Option<String>,

    /// Requested service tier; accepted and ignored (`docs/reference/inference.md`).
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
    service_tier: Option<String>,
}

/// Upstream prompt-cache options.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptCacheOptions {
    /// Cache placement mode.
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
    mode: Option<String>,

    /// Cache time-to-live hint.
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
    ttl: Option<String>,
}

/// Parses a standalone compact request body into IR with its compaction
/// control: the route itself is the control
/// ([`CompactionControl::ResponsesStandalone`]), the body never states one.
///
/// The request is always non-streaming and the input conversion is the
/// ordinary `/responses` path's own, so history carrying compaction and
/// reasoning state parses identically on both endpoints.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for malformed JSON, unknown or unsupported
/// fields, a present `previous_response_id` (stateful continuation is not
/// supported), a `compaction_trigger` item (the trigger belongs to the
/// ordinary endpoint), and every input-conversion rejection.
pub(crate) fn parse_compact_request(body: &[u8]) -> Result<ParsedRequest, InvalidRequest> {
    let request: CompactRequestBody = format::parse_client_request(body)?;

    if let Some(previous) = request.previous_response_id {
        return Err(InvalidRequest::field(
            "previous_response_id",
            format!(
                "Stateful continuation is not supported; the gateway is stateless, so \
                 previous_response_id `{previous}` cannot be resolved"
            ),
        ));
    }

    let input_items: Vec<ItemIn> = match request.input {
        StringOrArray::Text(text) => vec![ItemIn::user_text(text)],

        StringOrArray::Array(items) => items,
    };

    let mut converted = convert_input(input_items)?;
    if let Some(instructions) = &request.instructions {
        converted.instruction_items.insert(
            0,
            crate::format::ParsedInstructionItem {
                role: crate::compaction_model::identity::InstructionRole::System,
                text: instructions.clone(),
            },
        );
    }

    if converted.has_trigger {
        return Err(InvalidRequest::field(
            "input",
            "compaction_trigger is not valid on the compact endpoint",
        ));
    }

    Ok(ParsedRequest {
        affinity: crate::format::affinity::Sources::from_wire_messages(
            crate::format::UpstreamFormat::OpenAiResponse,
            &converted.messages,
        ),
        ir: IrRequest {
            state_items: converted.state_items,
            model: request.model,
            instructions: build_instructions(request.instructions, converted.instruction_parts),
            messages: converted.messages,
            ..IrRequest::default()
        },
        is_streaming: false,
        should_include_usage: false,
        compaction: Some(ParsedCompaction {
            control: Some(CompactionControl::ResponsesStandalone),
            boundaries: converted.boundaries,
            instruction_items: converted.instruction_items,
        }),
    })
}

/// Accepted replay state, preserving an omitted wire ID instead of inventing one.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename = "compaction")]
struct ReplayedCompactionItem {
    /// Provider-minted identity, only when the client actually supplied it.
    #[serde(skip_serializing_if = "OptionalReplayText::is_absent")]
    id: OptionalReplayText,
    /// Exact accepted native ciphertext or gateway carrier.
    encrypted_content: String,
    /// Authoring provenance, only when supplied.
    #[serde(skip_serializing_if = "OptionalReplayText::is_absent")]
    created_by: OptionalReplayText,
}

/// Converts accepted state without applying new-output identity generation.
///
/// # Panics
///
/// Panics if this concrete replay payload fails to serialize, an impossible serde contract.
pub(super) fn replayed_compaction_item_json(
    id: OptionalReplayText,
    encrypted_content: String,
    created_by: OptionalReplayText,
) -> Value {
    #[expect(
        clippy::expect_used,
        reason = "the concrete replay payload serializes totally"
    )]
    serde_json::to_value(ReplayedCompactionItem {
        id,
        encrypted_content,
        created_by,
    })
    .expect("the accepted compaction item serializes")
}

/// Builds a concrete gateway-generated compaction item.
pub(crate) fn compaction_item(
    id: Option<String>,
    encrypted_content: String,
    created_by: Option<String>,
) -> crate::format::compaction_wire::Checkpoint {
    crate::format::compaction_wire::Checkpoint::Responses(
        crate::format::compaction_wire::ResponsesCheckpoint {
            id: id.unwrap_or_else(|| format!("cmp_{}", ids::new_uuid())),
            encrypted_content,
            created_by,
        },
    )
}

/// Builds the JSON value of one replayed reasoning state item: the typed
/// members re-serialized with their presence preserved, under the item's
/// `reasoning` type tag.
///
/// # Panics
///
/// Panics if the item fails to serialize, which cannot occur for a first-party
/// serde type.
pub(super) fn reasoning_item_json(reasoning: &super::request_parse::ReasoningItem) -> Value {
    /// The serialization wrapper supplying the item's type tag; the tagged
    /// members ride flattened, so their presence is preserved exactly.
    #[derive(Serialize)]
    #[serde(tag = "type", rename = "reasoning")]
    struct ReasoningItemOut<'a> {
        /// The typed members.
        #[serde(flatten)]
        item: &'a super::request_parse::ReasoningItem,
    }

    #[expect(
        clippy::expect_used,
        reason = "a first-party rendered item serializes totally"
    )]
    serde_json::to_value(ReasoningItemOut { item: reasoning }).expect("a reasoning item serializes")
}

/// The standalone compact endpoint's success body: the `response.compaction`
/// object. The output is the canonical next window — the complete ordered
/// vendor-shaped item list — never reduced to its compaction item
/// (`docs/reference/configuration.md`). The body is render-only: the gateway constructs it, anchors
/// it, and serializes it outward, never parsing it back.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CompactedResponseBody {
    /// Gateway-issued response identifier.
    id: String,

    /// Always `response.compaction`.
    object: CompactedResponseObject,

    /// Unix creation timestamp in seconds.
    created_at: i64,

    /// The canonical next window, in output order.
    output: Vec<crate::format::compaction_wire::Checkpoint>,

    /// Token accounting of the calls that produced the window.
    usage: ResponseUsage,

    /// Sealer-proven identity retained only for this response's ephemeral registration.
    #[serde(skip)]
    gateway_artifact_id: Option<String>,
}

/// The only Responses item kind that establishes a standalone artifact hint.
#[derive(Serialize, Deserialize)]
enum ReplayedArtifactKind {
    /// Complete client-held compaction state.
    #[serde(rename = "compaction")]
    Compaction,
}

/// Borrowed accepted native artifact fields, excluding unsupported replay members.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayedArtifact<'a> {
    /// Closed native item discriminator, kept explicit beside strict accepted member parsing.
    #[serde(rename = "type")]
    kind: ReplayedArtifactKind,
    /// Wire identity, only when supplied.
    #[serde(borrow)]
    id: Option<&'a str>,
    /// Exact native state or sealed gateway token.
    encrypted_content: &'a str,
    /// Optional native authoring attribution.
    #[serde(borrow)]
    created_by: Option<&'a str>,
}

impl CompactedResponseBody {
    /// Projects the actual canonical next window independently of its discarded covered input.
    /// The codec borrows existing response text and state instead of rebuilding another prompt.
    pub(crate) fn window_anchors(&self) -> Vec<crate::affinity::Anchor> {
        completed_window_anchors(&self.output, self.gateway_artifact_id.is_some())
    }

    /// Identifies only the latest complete replayable outward artifact, without parsing prompts,
    /// copying retained text, unsealing our own carrier again, or trusting an outer resource ID.
    ///
    /// # Panics
    ///
    /// Panics if accepted native JSON fails to serialize to the infallible hashing sink.
    pub(crate) fn artifact_anchor(
        &self,
        projector: &crate::affinity::fingerprint::Projector,
    ) -> Option<crate::affinity::Anchor> {
        if let Some(artifact_id) = &self.gateway_artifact_id {
            return Some(crate::affinity::Anchor::Artifact(
                projector.gateway_artifact(artifact_id),
            ));
        }
        for checkpoint in self.output.iter().rev() {
            let crate::format::compaction_wire::Checkpoint::Native(item) = checkpoint else {
                continue;
            };
            let Ok(artifact) = ReplayedArtifact::deserialize(item) else {
                continue;
            };
            if crate::compaction_model::sealing::is_gateway_marker(artifact.encrypted_content) {
                // Native upstream output cannot supply the sealer-proven sidecar of our newly
                // emitted carrier. Never invent gateway provenance from a reflected marker.
                return None;
            }
            return Some(crate::affinity::Anchor::Artifact(
                projector.native_artifact(item),
            ));
        }
        None
    }
}

/// Projects a newly sealed gateway checkpoint's actual completed Responses output. Callers
/// must carry the sealer-proven identity separately; reflected upstream markers cannot use it.
pub(crate) fn gateway_window_anchors(
    output: &[crate::format::compaction_wire::Checkpoint],
) -> Vec<crate::affinity::Anchor> {
    completed_window_anchors(output, true)
}

/// Identifies a completed native artifact's actual accepted wire payload without provenance
/// inference. Reflected gateway markers require their authenticated sealer identity instead.
pub(super) fn native_artifact_anchor(
    item: &Value,
    projector: &crate::affinity::fingerprint::Projector,
) -> Option<crate::affinity::Anchor> {
    let artifact = ReplayedArtifact::deserialize(item).ok()?;
    if crate::compaction_model::sealing::is_gateway_marker(artifact.encrypted_content) {
        return None;
    }
    Some(crate::affinity::Anchor::Artifact(
        projector.native_artifact(item),
    ))
}

/// Derives complete and latest-active history anchors without reconstructing response text.
fn completed_window_anchors(
    output: &[crate::format::compaction_wire::Checkpoint],
    has_gateway_provenance: bool,
) -> Vec<crate::affinity::Anchor> {
    let Some(window) = super::window_affinity::project(output, has_gateway_provenance, false)
    else {
        return Vec::new();
    };
    let mut anchors = window.completed_anchors();
    // Also register the latest active window used by routing preparation, which omits the
    // preceding covered units, including any older accepted alias state. Only a
    // fully replayable complete returned window can establish either history projection.
    if let Some(active) = super::window_affinity::project(output, has_gateway_provenance, true) {
        anchors.extend(active.completed_anchors());
    }
    anchors
}

/// Renders a native compacted upstream result into the compact endpoint's
/// success body: the upstream's identity, creation time, complete ordered
/// output, and usage — with its `dropped_message_count` — carried through.
pub(crate) fn render_compacted_response(native: CompactedResponseNative) -> CompactedResponseBody {
    CompactedResponseBody {
        id: native
            .id
            .unwrap_or_else(|| format!("resp_{}", ids::new_uuid())),
        object: CompactedResponseObject::Compaction,
        created_at: native.created_at.unwrap_or_else(unix_time::unix_now),
        output: native.output.into_iter().map(Into::into).collect(),
        usage: ResponseUsage::render_upstream(native.usage.unwrap_or_default()),
        gateway_artifact_id: None,
    }
}

/// Renders a gateway-produced compaction into the compact endpoint's success
/// body: one `compaction` item carrying the sealed gateway carrier, with the
/// summary call's usage and zero-filled breakdowns.
pub(crate) fn render_gateway_compacted_response(
    carrier: String,
    artifact_id: String,
    usage: IrUsage,
) -> CompactedResponseBody {
    CompactedResponseBody {
        id: format!("resp_{}", ids::new_uuid()),
        object: CompactedResponseObject::Compaction,
        created_at: unix_time::unix_now(),
        output: vec![compaction_item(None, carrier, None)],
        usage: ResponseUsage::render(usage),
        gateway_artifact_id: Some(artifact_id),
    }
}

/// Renders the explicit trigger lifecycle's complete SSE frame sequence:
/// `response.created`, `response.in_progress`, one `output_item.added` and
/// `output_item.done` pair per prefix item, then `response.completed` carrying
/// `usage` — the framing Codex requires (`docs/design/compaction.md`), rendered through
/// the ordinary [`SseRenderer`] so the envelope, item, and usage shapes cannot
/// drift from the ordinary lifecycle's.
pub(crate) fn render_trigger_stream_frames(
    model: String,
    prefix_items: Vec<crate::format::compaction_wire::Checkpoint>,
    usage: IrUsage,
) -> Vec<String> {
    let mut renderer = SseRenderer::new(model).with_prefix_items(prefix_items);

    let mut frames = renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    });

    frames.extend(renderer.render(&IrEvent::Usage(usage)));
    frames.extend(renderer.render(&IrEvent::Done));

    frames
}
