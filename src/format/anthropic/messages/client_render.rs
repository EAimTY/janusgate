//! Client-response rendering for the Anthropic Messages codec: renders IR events as native
//! Messages SSE frames (`message_start`, `content_block_*`, `message_delta`, `message_stop`) or an
//! aggregated non-streaming response.

use super::{OutBlock, TypedBlock, is_anthropic_block};
use crate::{
    api::error::anthropic::{AnthropicErrorBody, render_body},
    format::{ClientSseRenderer, arguments_to_object_or_empty},
    ids,
    ir::{IrAggregation, IrEvent, IrFinishReason, IrUsage},
    sse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The aggregated, non-streaming Messages response body.
#[derive(Debug, Serialize)]
pub(crate) struct MessagesResponse {
    /// Gateway-issued message identifier.
    id: String,

    /// Always `message`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// Always `assistant`.
    role: &'static str,

    /// Response content: reasoning blocks first — replayable same-family blocks verbatim,
    /// every other reasoning block's text as an ordinary text block (README §5.8) — then the
    /// answer text, then tool calls.
    content: Vec<OutBlock>,

    /// Public model name the client addressed.
    model: String,

    /// Canonical stop reason.
    stop_reason: &'static str,

    /// No sequence is ever stopped on: always null.
    stop_sequence: Option<String>,

    /// Token accounting.
    usage: ResponseUsage,
}

/// Token accounting in the native Messages shape: totals always, cache columns when reported, and
/// the output reasoning breakdown when the upstream reported it. A compaction lifecycle
/// additionally renders the raw vendor iteration entries under `usage.iterations`; ordinary
/// responses render none.
#[derive(Debug, Serialize)]
struct ResponseUsage {
    /// Tokens consumed by the input.
    input_tokens: u64,

    /// Tokens produced by the output.
    output_tokens: u64,

    /// Input tokens written to the upstream prompt cache, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_creation_input_tokens: Option<u64>,

    /// Input tokens served from the upstream prompt cache, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_read_input_tokens: Option<u64>,

    /// Output token breakdown, when the upstream reported one — the native spelling of the IR
    /// reasoning column, omitted when absent (README §5.8).
    #[serde(skip_serializing_if = "Option::is_none")]
    output_tokens_details: Option<OutputTokensDetails>,

    /// Raw vendor iteration entries of a compaction lifecycle, one per sampling iteration the
    /// compaction performed; omitted on every ordinary response, whose usage carries no
    /// iterations.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    iterations: Vec<crate::ir::compaction::UsageIteration>,
}

/// The native output token breakdown.
#[derive(Debug, Serialize)]
struct OutputTokensDetails {
    /// Output tokens spent on extended thinking.
    thinking_tokens: u64,
}

impl ResponseUsage {
    /// Renders one observation, carrying reported cache columns and the reasoning breakdown and
    /// omitting the rest.
    fn render(usage: IrUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_input_tokens: usage.cache_write_tokens,
            cache_read_input_tokens: usage.cached_tokens,
            output_tokens_details: usage
                .reasoning_tokens
                .map(|thinking_tokens| OutputTokensDetails { thinking_tokens }),
            iterations: Vec::new(),
        }
    }

    /// Zero usage, opening the streamed message before any report arrived.
    fn zero() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            output_tokens_details: None,
            iterations: Vec::new(),
        }
    }

    /// Opens the streamed message from a stream-head partial report: its input and cache
    /// columns ride the opening usage, while the output stays zero until a terminal report
    /// overwrites the partial one (README §6.2).
    fn from_head_report(usage: IrUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: 0,
            cache_creation_input_tokens: usage.cache_write_tokens,
            cache_read_input_tokens: usage.cached_tokens,
            output_tokens_details: None,
            iterations: Vec::new(),
        }
    }
}

/// Usage facts of a compaction-only response: the raw vendor iteration entries rendered under
/// `usage.iterations`, and the top-level columns the protocol renders beside them — zero for a
/// compact-only reply, whose compaction iterations are the billed work.
#[derive(Debug, Clone)]
pub(crate) struct CompactionOnlyUsage {
    /// Raw vendor iteration entries to render under `usage.iterations`, verbatim.
    pub(crate) iterations: Vec<crate::ir::compaction::UsageIteration>,

    /// Top-level input tokens; zero for a compact-only reply per the protocol.
    pub(crate) input_tokens: u64,

    /// Top-level output tokens; zero for a compact-only reply per the protocol.
    pub(crate) output_tokens: u64,

    /// Input tokens written to the upstream prompt cache, when reported.
    pub(crate) cache_write_tokens: Option<u64>,

    /// Input tokens served from the upstream prompt cache, when reported.
    pub(crate) cached_tokens: Option<u64>,
}

/// Builds the concrete iteration report for an actual ordinary generation dispatch.
fn message_iteration_entry(usage: IrUsage) -> crate::ir::compaction::UsageIteration {
    crate::ir::compaction::UsageIteration::gateway(
        crate::ir::compaction::SamplingKind::Message,
        usage,
    )
}

/// Renders a compaction-only Messages response: the content carries exactly the given
/// `compaction` block, the stop reason is `"compaction"`, and the usage renders the given
/// top-level columns — zero for a compact-only reply per the protocol — with the compaction
/// iteration entries under `usage.iterations`. The compaction executor calls this for a
/// compact-only lifecycle's aggregated body.
pub(crate) fn render_compaction_only_response(
    public_id: &str,
    block: crate::format::compaction_wire::Checkpoint,
    usage: &CompactionOnlyUsage,
) -> MessagesResponse {
    MessagesResponse {
        id: format!("msg_{}", ids::new_uuid()),
        kind: "message",
        role: "assistant",
        content: vec![OutBlock::Checkpoint(block)],
        model: public_id.to_string(),
        stop_reason: "compaction",
        stop_sequence: None,
        usage: ResponseUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_input_tokens: usage.cache_write_tokens,
            cache_read_input_tokens: usage.cached_tokens,
            output_tokens_details: None,
            iterations: usage.iterations.clone(),
        },
    }
}

/// Renders an aggregated completion as a non-streaming Messages response.
/// Upstream-observed native compaction state blocks ride the content head, in
/// their arrival order, ahead of the derived blocks — a native compaction
/// precedes the answer output it precedes upstream.
pub(super) fn render_json_response(model: &str, aggregation: &IrAggregation) -> MessagesResponse {
    let mut content: Vec<OutBlock> = Vec::new();

    for block in &aggregation.compaction_states {
        content.push(OutBlock::Verbatim(block.clone()));
    }

    for block in &aggregation.reasoning_blocks {
        if is_anthropic_block(&block.opaque) {
            content.push(OutBlock::Verbatim(block.opaque.clone()));
        } else if !block.text.is_empty() {
            // Not family-replayable — a foreign-family block, or a signature-less `thinking`
            // shape the official schema forbids — so the reasoning text rides the ordinary text
            // channel: conformant on every surface and replayable without a trap, where a
            // synthesized `thinking` block without its signature is neither (README §5.8).
            content.push(OutBlock::Typed(TypedBlock::Text {
                text: block.text.clone(),
            }));
        }
    }

    if !aggregation.text.is_empty() {
        content.push(OutBlock::Typed(TypedBlock::Text {
            text: aggregation.text.clone(),
        }));
    }

    for tool_call in &aggregation.tool_calls {
        content.push(OutBlock::Typed(TypedBlock::ToolUse {
            id: tool_call.call_id.clone(),
            name: tool_call.name.clone(),
            input: Value::Object(arguments_to_object_or_empty(&tool_call.arguments)),
        }));
    }

    MessagesResponse {
        id: format!("msg_{}", ids::new_uuid()),
        kind: "message",
        role: "assistant",
        content,
        model: model.to_string(),
        stop_reason: if aggregation.is_compaction_paused {
            "compaction"
        } else {
            aggregation
                .finish
                .map(finish_reason_to_stop)
                .unwrap_or("end_turn")
        },
        stop_sequence: None,
        usage: {
            let mut usage = aggregation
                .usage
                .map(ResponseUsage::render)
                .unwrap_or_else(ResponseUsage::zero);
            usage.iterations = aggregation.compaction_iterations.clone();
            usage
        },
    }
}
/// Renders an aggregated completion whose content opens with the compaction
/// checkpoint blocks, in order, ahead of the derived blocks, and whose usage
/// carries the compaction lifecycle's vendor iteration entries — the
/// threshold-continue lifecycle's aggregated body.
pub(super) fn render_json_response_with_prefix_blocks(
    model: &str,
    aggregation: &IrAggregation,
    blocks: &[crate::format::compaction_wire::Checkpoint],
    iterations: &[crate::ir::compaction::UsageIteration],
) -> MessagesResponse {
    let mut response = render_json_response(model, aggregation);
    if blocks.is_empty() && iterations.is_empty() {
        return response;
    }

    let mut content: Vec<OutBlock> = blocks
        .iter()
        .map(|block| OutBlock::Checkpoint(block.clone()))
        .collect();

    content.append(&mut response.content);

    response.content = content;

    // A prefixed body is a compact-and-continue lifecycle: the compaction
    // iterations ride beside the message stage's own entry, whose usage is the
    // aggregated observation (README §6.8).
    let mut iterations = iterations.to_vec();

    if !aggregation.compaction_iterations.is_empty() {
        iterations.extend(aggregation.compaction_iterations.clone());
    } else if let Some(usage) = aggregation.usage {
        iterations.push(message_iteration_entry(usage));
    }

    response.usage.iterations = iterations;

    response
}

/// Renders the canonical reason as an Anthropic `stop_reason` string.
fn finish_reason_to_stop(reason: IrFinishReason) -> &'static str {
    match reason {
        IrFinishReason::Stop => "end_turn",
        IrFinishReason::Length => "max_tokens",
        IrFinishReason::ToolCall => "tool_use",
        IrFinishReason::ContentFilter => "refusal",
    }
}

/// Typed probe of a vendor compaction block's string content, deciding whether the block's
/// prefix rendering adds a `compaction_delta`: a block whose `content` is a string carries one,
/// a null-content no-op block carries none.
#[derive(Debug, Deserialize)]
struct CompactionContentProbe {
    /// The block's string content, when it carries one.
    #[serde(default)]
    content: Option<String>,
}

/// Reads the string content of a vendor compaction block: [`None`] when the block carries no
/// string `content` member — a null-content no-op, or a shape without the member at all.
fn compaction_block_content(block: &crate::format::compaction_wire::Checkpoint) -> Option<String> {
    use crate::format::compaction_wire::Checkpoint;
    match block {
        Checkpoint::Messages(block) => Some(block.content.clone()),
        Checkpoint::Native(value) => CompactionContentProbe::deserialize(value)
            .ok()
            .and_then(|probe| probe.content),
        Checkpoint::Responses(_) => None,
    }
}

/// A compaction opening block, with a null-content threshold form when the summary follows.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum CompactionStart {
    /// An on-demand or no-op block delivered complete at the start.
    Complete(crate::format::compaction_wire::Checkpoint),
    /// Concrete gateway threshold state.
    Gateway(ThresholdStart),
    /// Native metadata that cannot be discarded while replacing its readable content.
    Native(Value),
}

/// The concrete gateway threshold block opening, before its complete summary delta.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename = "compaction")]
struct ThresholdStart {
    /// Always null: the subsequent delta carries the summary once.
    content: Option<String>,
    /// Original authenticated gateway carrier.
    signature: String,
}

/// Preserves opaque native metadata while moving summary text to the threshold delta.
///
/// # Panics
///
/// Panics if a foreign JSON fragment fails to serialize, which cannot occur.
fn threshold_block_start(block: &crate::format::compaction_wire::Checkpoint) -> CompactionStart {
    use crate::format::compaction_wire::Checkpoint;
    match block {
        Checkpoint::Messages(block) => CompactionStart::Gateway(ThresholdStart {
            content: None,
            signature: block.signature.clone(),
        }),
        Checkpoint::Native(value) => {
            #[derive(Serialize)]
            struct NativeStart<'a> {
                /// Complete producer-owned metadata, with the content replaced below.
                #[serde(flatten)]
                original: &'a Value,
                /// Summary is delivered by a separate compaction delta.
                content: Option<String>,
            }
            #[expect(
                clippy::expect_used,
                reason = "a foreign JSON fragment serializes totally"
            )]
            let start = serde_json::to_value(NativeStart {
                original: value,
                content: None,
            })
            .expect("native block serializes");
            CompactionStart::Native(start)
        }
        Checkpoint::Responses(_) => CompactionStart::Complete(block.clone()),
    }
}

/// Streaming client response renderer producing Messages SSE frames.
pub(crate) struct SseRenderer {
    /// Content-free projection of the actual emitted conversation blocks.
    projection: Option<super::affinity::Output>,

    /// Last completed prepared-prefix frame in the current render batch.
    prefix_boundary: Option<usize>,

    /// Digest-only native checkpoint awaiting its actual stop-frame handoff.
    completed_artifact: Option<crate::format::CompletedArtifact>,

    /// Whether authenticated checkpoint fields already seed the active projection.
    is_prefix_prepared: bool,

    /// Gateway-issued message identifier.
    id: String,

    /// Public model name echoed to the client.
    model: String,

    /// Next stream content index to assign.
    next_index: usize,

    /// Currently open content block, closed lazily when the next block starts or the turn ends.
    open_block: Option<OpenOutBlock>,

    /// Whether the stream's [`IrEvent::MessageStart`] was observed; the gateway sends exactly
    /// one, and a repeated one renders nothing.
    is_started: bool,

    /// Whether the `message_start` frame is buffered awaiting the one-event lookahead: the
    /// frame emits when the next event resolves it, folding the stream-head partial usage
    /// report when that event is exactly the report (README §6.2).
    is_start_buffered: bool,

    /// Compaction blocks rendered as the stream's leading prefix, whole block per
    /// `content_block_start`, one full-summary `compaction_delta` per block with string
    /// content, then `content_block_stop` — before any ordinary block. Drained right after the
    /// `message_start` head resolves.
    pending_prefix: Vec<crate::format::compaction_wire::Checkpoint>,

    /// Whether summary content belongs in a threshold compaction delta.
    is_threshold_compaction: bool,

    /// Native cumulative iteration reports, separate from gateway prefix iterations.
    native_iterations: Vec<crate::ir::compaction::UsageIteration>,

    /// Stop reason forced onto the terminal `message_delta`, set for a compaction lifecycle
    /// whose outcome is the compaction stop (`"compaction"`); the finish-derived reason stands
    /// when absent.
    forced_stop_reason: Option<String>,

    /// Tool calls buffered from deltas, emitted whole on their done event so the `tool_use`
    /// start event carries its authoritative identity.
    tool_buffers: Vec<ToolBuffer>,

    /// Latest token accounting observed; rendered in the terminal delta.
    usage: Option<IrUsage>,

    /// The vendor-shaped iteration entries of a compaction lifecycle, rendered
    /// under the terminal delta's `usage.iterations` (README §6.8).
    compaction_iterations: Vec<crate::ir::compaction::UsageIteration>,

    /// Whether the renderer carries a compaction lifecycle — a prefix, a
    /// forced stop, or carried iteration entries — whose terminal delta closes
    /// the rendered iterations with the message stage's entry. Ordinary
    /// streams render no iterations at all.
    is_compaction_lifecycle: bool,

    /// Finish reason reported before [`IrEvent::Done`].
    finish: Option<IrFinishReason>,
}

/// A content block the renderer currently streams.
#[derive(Debug)]
enum OpenOutBlock {
    /// Plain text block at the stream index.
    Text {
        /// Stream content index of the block.
        index: usize,
    },

    /// Thinking block at the stream index.
    Thinking {
        /// Stream content index of the block.
        index: usize,
    },
}

/// One tool call buffered while its deltas stream in.
#[derive(Debug)]
struct ToolBuffer {
    /// Stable IR call index.
    ir_index: usize,

    /// Invocation identifier, latest revealed wins.
    call_id: Option<String>,

    /// Function name, latest revealed wins.
    name: Option<String>,

    /// Argument bytes assembled so far.
    arguments: String,
}

/// The streamed message head opening a `message_start` event.
#[derive(Debug, Serialize)]
struct StreamMessageHead {
    /// Gateway-issued message identifier.
    id: String,

    /// Always `message`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// Always `assistant`.
    role: &'static str,

    /// Always empty: blocks stream in after the start.
    content: [(); 0],

    /// Public model name echoed to the client.
    model: String,

    /// Never stopped yet at the start: always null.
    stop_reason: Option<String>,

    /// Never stopped on: always null.
    stop_sequence: Option<String>,

    /// Zero usage; the terminal delta carries the observed report. A stream-head partial
    /// report folds its input and cache columns in when the upstream published one.
    usage: ResponseUsage,
}

/// The `message_start` event payload.
#[derive(Debug, Serialize)]
struct MessageStartPayload {
    /// Always `message_start`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// The streamed message head.
    message: StreamMessageHead,
}

/// A block shape opening a `content_block_start` event.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StartBlockOut {
    /// Text block opening empty.
    Text {
        /// Always empty at the start.
        text: &'static str,
    },

    /// Thinking block opening empty; the signature streams later as a delta. The official
    /// `ThinkingBlock` shape requires the `signature` member, so the start frame carries it
    /// empty until the `signature_delta` lands (README §5.8).
    Thinking {
        /// Always empty at the start.
        thinking: &'static str,

        /// Always empty at the start: the real signature streams later as a delta.
        signature: &'static str,
    },

    /// Tool invocation block with its identity; arguments stream as deltas.
    ToolUse {
        /// Invocation identifier.
        id: String,

        /// Invoked function name.
        name: String,

        /// Always empty at the start.
        input: crate::format::EmptyObject,
    },
}

/// The `content_block_start` event payload.
#[derive(Debug, Serialize)]
struct ContentBlockStartPayload {
    /// Always `content_block_start`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// Stream content index of the block.
    index: usize,

    /// Opening block shape.
    content_block: StartBlockOut,
}

/// The `content_block_start` event payload carrying a whole vendor block verbatim — the
/// compaction prefix's start frame, whose block shape is the vendor's to define.
#[derive(Debug, Serialize)]
struct VerbatimBlockStartPayload {
    /// Always `content_block_start`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// Stream content index of the block.
    index: usize,

    /// The whole vendor block, verbatim.
    content_block: CompactionStart,
}

/// One streamed content delta.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum DeltaOut {
    /// Assistant text bytes.
    #[serde(rename = "text_delta")]
    Text {
        /// Text bytes.
        text: String,
    },

    /// Reasoning text bytes.
    #[serde(rename = "thinking_delta")]
    Thinking {
        /// Reasoning bytes.
        thinking: String,
    },

    /// Signature terminating a thinking block.
    #[serde(rename = "signature_delta")]
    Signature {
        /// Upstream signature.
        signature: String,
    },

    /// Tool argument bytes.
    #[serde(rename = "input_json_delta")]
    InputJson {
        /// JSON bytes.
        partial_json: String,
    },

    /// The complete compaction summary in one full-content delta: a compaction block never
    /// streams ordinary text deltas, so its single delta carries the whole readable content.
    #[serde(rename = "compaction_delta")]
    Compaction {
        /// The complete summary content.
        content: String,
    },
}

/// The `content_block_delta` event payload.
#[derive(Debug, Serialize)]
struct ContentBlockDeltaPayload {
    /// Always `content_block_delta`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// Stream content index of the block.
    index: usize,

    /// The delta itself.
    delta: DeltaOut,
}

/// The `content_block_stop` event payload.
#[derive(Debug, Serialize)]
struct ContentBlockStopPayload {
    /// Always `content_block_stop`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// Stream content index of the block.
    index: usize,
}

/// Stop-reason head of an outbound `message_delta` event.
#[derive(Debug, Serialize)]
struct MessageDelta {
    /// Canonical stop reason, or the forced compaction stop reason of a compaction lifecycle.
    stop_reason: String,

    /// Never stopped on: always null.
    stop_sequence: Option<String>,
}

/// The `message_delta` event payload.
#[derive(Debug, Serialize)]
struct MessageDeltaPayload {
    /// Always `message_delta`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// Stop-reason head.
    delta: MessageDelta,

    /// Terminal token accounting.
    usage: ResponseUsage,
}

/// The `message_stop` event payload.
#[derive(Debug, Serialize)]
struct MessageStopPayload {
    /// Always `message_stop`.
    #[serde(rename = "type")]
    kind: &'static str,
}

/// The mid-stream `error` event payload, classified once through the shared API error.
#[derive(Debug, Serialize)]
struct StreamErrorPayload {
    /// Always `error`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// Rendered error body.
    error: AnthropicErrorBody,
}

impl SseRenderer {
    /// Creates a renderer for one streamed completion.
    pub(crate) fn new(model: String) -> Self {
        Self {
            id: format!("msg_{}", ids::new_uuid()),
            projection: None,
            prefix_boundary: None,
            completed_artifact: None,
            is_prefix_prepared: false,
            model,
            next_index: 0,
            open_block: None,
            is_started: false,
            is_start_buffered: false,
            pending_prefix: Vec::new(),
            is_threshold_compaction: false,
            native_iterations: Vec::new(),
            forced_stop_reason: None,
            tool_buffers: Vec::new(),
            usage: None,
            compaction_iterations: Vec::new(),
            is_compaction_lifecycle: false,
            finish: None,
        }
    }

    /// Carries the lifecycle's vendor-shaped iteration entries into the
    /// terminal `message_delta`'s `usage.iterations` (README §6.8) and
    /// marks the stream a compaction lifecycle.
    #[must_use]
    pub(crate) fn with_usage_iterations(
        mut self,
        iterations: Vec<crate::ir::compaction::UsageIteration>,
    ) -> Self {
        self.is_compaction_lifecycle = self.is_compaction_lifecycle || !iterations.is_empty();
        self.compaction_iterations = iterations;

        self
    }

    /// Creates a renderer for one streamed compaction lifecycle: `blocks` render as the
    /// stream's leading compaction prefix — whole block per `content_block_start`, one
    /// full-summary `compaction_delta` per block carrying string content, then
    /// `content_block_stop`, indices `0..n` ahead of every ordinary block — and
    /// `forced_stop_reason`, when set, replaces the terminal `message_delta`'s finish-derived
    /// stop reason (the `"compaction"` pause/compact-only outcome).
    pub(crate) fn with_compaction_prefix(
        model: String,
        blocks: Vec<crate::format::compaction_wire::Checkpoint>,
        forced_stop_reason: Option<String>,
    ) -> Self {
        Self {
            pending_prefix: blocks,
            forced_stop_reason,
            is_compaction_lifecycle: true,
            is_threshold_compaction: true,
            ..Self::new(model)
        }
    }

    /// Selects threshold delta framing or on-demand complete-block framing.
    pub(crate) const fn with_threshold_compaction(mut self, is_threshold_compaction: bool) -> Self {
        self.is_threshold_compaction = is_threshold_compaction;
        self
    }

    /// Formats one SSE frame with its event name and typed payload.
    fn frame<T>(event: &'static str, payload: &T) -> String
    where
        T: Serialize + ?Sized,
    {
        sse::format_frame(Some(event), &crate::format::render_json(payload))
    }

    /// Drains the compaction prefix, rendering each pending block at the next stream content
    /// index: the whole block rides `content_block_start` verbatim, a block carrying string
    /// content adds one full-content `compaction_delta`, and every block closes with
    /// `content_block_stop`. Ordinary blocks continue after the prefix indices.
    fn drain_compaction_prefix(&mut self) -> Vec<String> {
        let blocks = std::mem::take(&mut self.pending_prefix);

        let mut frames = Vec::new();

        for block in &blocks {
            let index = self.next_index;

            self.next_index += 1;

            // The start- and delta-frame decisions share one probe, so the two framings cannot
            // disagree per block: a block with string content rides the threshold start plus
            // its summary delta, every other block delivers the whole block in its start.
            let delta_content = if self.is_threshold_compaction {
                compaction_block_content(block)
            } else {
                None
            };

            frames.push(Self::frame(
                "content_block_start",
                &VerbatimBlockStartPayload {
                    kind: "content_block_start",
                    index,
                    content_block: if delta_content.is_some() {
                        threshold_block_start(block)
                    } else {
                        CompactionStart::Complete(block.clone())
                    },
                },
            ));

            // A block without string content — the native null-content no-op — carries no
            // delta: its start already delivered the whole block.
            if let Some(content) = delta_content {
                frames.push(Self::frame(
                    "content_block_delta",
                    &ContentBlockDeltaPayload {
                        kind: "content_block_delta",
                        index,
                        delta: DeltaOut::Compaction { content },
                    },
                ));
            }

            frames.push(Self::frame(
                "content_block_stop",
                &ContentBlockStopPayload {
                    kind: "content_block_stop",
                    index,
                },
            ));
        }

        frames
    }

    /// Renders one upstream-observed native compaction state block: identical
    /// shape to the compaction prefix (start, optional full-content
    /// `compaction_delta`, stop) at the next stream content index.
    fn render_compaction_state_block(&mut self, payload: &Value) -> Vec<String> {
        // State is a new content block. Closing the prior text/reasoning block keeps subsequent
        // answer deltas at positions after this checkpoint, exactly as ordinary replay sees them.
        let mut frames = self.close_open_block();
        self.pending_prefix.push(payload.clone().into());
        frames.extend(self.drain_compaction_prefix());
        frames
    }

    /// Closes the currently open block, if any.
    fn close_open_block(&mut self) -> Vec<String> {
        let Some(open) = self.open_block.take() else {
            return Vec::new();
        };

        let index = match open {
            OpenOutBlock::Text { index } | OpenOutBlock::Thinking { index } => index,
        };

        vec![Self::frame(
            "content_block_stop",
            &ContentBlockStopPayload {
                kind: "content_block_stop",
                index,
            },
        )]
    }

    /// Opens a text block, closing whatever is open; returns the start frame when a new block
    /// opened.
    fn ensure_text_block(&mut self) -> Vec<String> {
        if matches!(self.open_block, Some(OpenOutBlock::Text { .. })) {
            return Vec::new();
        }

        let mut frames = self.close_open_block();
        let index = self.next_index;

        self.next_index += 1;
        self.open_block = Some(OpenOutBlock::Text { index });

        frames.push(Self::frame(
            "content_block_start",
            &ContentBlockStartPayload {
                kind: "content_block_start",
                index,
                content_block: StartBlockOut::Text { text: "" },
            },
        ));

        frames
    }

    /// Opens a thinking block, closing whatever is open; returns the start frame when a new
    /// block opened.
    fn ensure_thinking_block(&mut self) -> Vec<String> {
        if matches!(self.open_block, Some(OpenOutBlock::Thinking { .. })) {
            return Vec::new();
        }

        let mut frames = self.close_open_block();
        let index = self.next_index;

        self.next_index += 1;
        self.open_block = Some(OpenOutBlock::Thinking { index });

        frames.push(Self::frame(
            "content_block_start",
            &ContentBlockStartPayload {
                kind: "content_block_start",
                index,
                content_block: StartBlockOut::Thinking {
                    thinking: "",
                    signature: "",
                },
            },
        ));

        frames
    }

    /// Finds the tool buffer for an IR call index, opening one when the first delta for the
    /// index arrives.
    ///
    /// # Panics
    ///
    /// Panics if the newly pushed buffer cannot be found, which cannot occur.
    fn tool_buffer(&mut self, ir_index: usize) -> &mut ToolBuffer {
        if !self
            .tool_buffers
            .iter()
            .any(|buffer| buffer.ir_index == ir_index)
        {
            self.tool_buffers.push(ToolBuffer {
                ir_index,
                call_id: None,
                name: None,
                arguments: String::new(),
            });
        }

        #[expect(
            clippy::expect_used,
            reason = "the buffer was just pushed when missing"
        )]
        self.tool_buffers
            .iter_mut()
            .find(|buffer| buffer.ir_index == ir_index)
            .expect("the tool buffer exists after insertion")
    }

    /// Builds the `message_start` frame carrying the given opening usage.
    fn message_start_frame(&self, usage: ResponseUsage) -> String {
        Self::frame(
            "message_start",
            &MessageStartPayload {
                kind: "message_start",
                message: StreamMessageHead {
                    id: self.id.clone(),
                    kind: "message",
                    role: "assistant",
                    content: [],
                    model: self.model.clone(),
                    stop_reason: None,
                    stop_sequence: None,
                    usage,
                },
            },
        )
    }

    /// Emits the terminal `message_delta` frame carrying the stop reason and the final usage.
    /// Rendered at [`IrEvent::Done`], not at [`IrEvent::Finish`]: cross-format upstreams
    /// deliver usage after the finish reason, so only Done-time usage is final. A compaction
    /// lifecycle's forced stop reason replaces the finish-derived one, and its iteration
    /// entries ride the usage.
    fn message_delta_frame(&self, reason: IrFinishReason) -> String {
        let stop_reason = self
            .forced_stop_reason
            .clone()
            .unwrap_or_else(|| finish_reason_to_stop(reason).to_string());

        let mut usage = self
            .usage
            .map(ResponseUsage::render)
            .unwrap_or_else(|| ResponseUsage::render(IrUsage::default()));

        usage.iterations = self.compaction_iterations.clone();
        usage.iterations.extend(self.native_iterations.clone());

        // A streamed compact-and-continue lifecycle reports the message
        // stage's usage at its terminal: its entry closes the rendered
        // iterations (README §6.8). A compact-only lifecycle reports no
        // usage observation, and an ordinary stream is no lifecycle at all —
        // neither fabricates an entry.
        if self.is_compaction_lifecycle
            && self.native_iterations.is_empty()
            && let Some(observed) = self.usage
        {
            usage.iterations.push(message_iteration_entry(observed));
        }

        Self::frame(
            "message_delta",
            &MessageDeltaPayload {
                kind: "message_delta",
                delta: MessageDelta {
                    stop_reason,
                    stop_sequence: None,
                },
                usage,
            },
        )
    }
}

impl SseRenderer {
    /// Renders one IR event into zero or more SSE frames.
    fn render_frames(&mut self, event: &IrEvent) -> Vec<String> {
        // One-event lookahead: the head stays buffered until the next event resolves it, so
        // the stream-head partial usage report — published immediately after
        // [`IrEvent::MessageStart`] per the IR contract (README §6.2) — folds its input and
        // cache columns into the opening usage; every other event resolves the buffer with
        // zeros first, keeping `message_start` exactly once and first in every stream.
        let mut frames = Vec::new();
        self.prefix_boundary = None;

        if self.is_start_buffered {
            self.is_start_buffered = false;

            let usage = match event {
                IrEvent::Usage(reported) => ResponseUsage::from_head_report(*reported),
                _ => ResponseUsage::zero(),
            };

            frames.push(self.message_start_frame(usage));

            // The compaction prefix rides immediately after the head, ahead of every ordinary
            // block the resolving event opens.
            let prefix = self.drain_compaction_prefix();
            if !prefix.is_empty() {
                self.prefix_boundary = Some(frames.len() + prefix.len() - 1);
            }
            frames.extend(prefix);
        }

        frames.extend(match event {
            IrEvent::MessageStart { .. } => {
                if !self.is_started {
                    self.is_started = true;
                    // Buffered one event: the resolving event decides between the partial
                    // report's columns and zeros — the lookahead above.
                    self.is_start_buffered = true;
                }

                Vec::new()
            }

            IrEvent::CompactionState { payload } => self.render_compaction_state_block(payload),

            IrEvent::TextDelta(delta) => {
                let mut frames = self.ensure_text_block();

                let Some(OpenOutBlock::Text { index }) = &self.open_block else {
                    return frames;
                };

                let index = *index;

                frames.push(Self::frame(
                    "content_block_delta",
                    &ContentBlockDeltaPayload {
                        kind: "content_block_delta",
                        index,
                        delta: DeltaOut::Text {
                            text: delta.clone(),
                        },
                    },
                ));

                frames
            }

            IrEvent::ReasoningDelta(delta) => {
                let mut frames = self.ensure_thinking_block();

                let Some(OpenOutBlock::Thinking { index }) = &self.open_block else {
                    return frames;
                };

                let index = *index;

                frames.push(Self::frame(
                    "content_block_delta",
                    &ContentBlockDeltaPayload {
                        kind: "content_block_delta",
                        index,
                        delta: DeltaOut::Thinking {
                            thinking: delta.clone(),
                        },
                    },
                ));

                frames
            }

            // A done carrying text renders it only when no deltas preceded it: upstream parsers
            // emit either deltas plus an empty-text done, or a done with the full text and no
            // deltas, never both with overlapping text. The opaque payload stays server-side on
            // the streaming path — deltas already delivered the text — and the aggregated
            // rendering replays it.
            IrEvent::ReasoningDone {
                text, signature, ..
            } => {
                let mut frames = Vec::new();

                if !text.is_empty() {
                    frames.extend(self.ensure_thinking_block());

                    let Some(OpenOutBlock::Thinking { index }) = &self.open_block else {
                        return frames;
                    };

                    let index = *index;

                    frames.push(Self::frame(
                        "content_block_delta",
                        &ContentBlockDeltaPayload {
                            kind: "content_block_delta",
                            index,
                            delta: DeltaOut::Thinking {
                                thinking: text.clone(),
                            },
                        },
                    ));
                }

                if let Some(signature) = signature {
                    // A signature without an open block cannot anchor a delta; the invariant
                    // above means a signature always accompanies streamed or full text, so the
                    // block is open here on every conformant stream.
                    if let Some(OpenOutBlock::Thinking { index }) = &self.open_block {
                        let index = *index;

                        frames.push(Self::frame(
                            "content_block_delta",
                            &ContentBlockDeltaPayload {
                                kind: "content_block_delta",
                                index,
                                delta: DeltaOut::Signature {
                                    signature: signature.clone(),
                                },
                            },
                        ));
                    }
                }

                frames.extend(self.close_open_block());

                frames
            }

            // Tool deltas buffer client-side: the `tool_use` start event carries its
            // authoritative identity, which only the done event knows completely.
            IrEvent::ToolCallDelta {
                index,
                call_id,
                name,
                arguments_delta,
            } => {
                let buffer = self.tool_buffer(*index);

                if call_id.is_some() {
                    buffer.call_id.clone_from(call_id);
                }

                if name.is_some() {
                    buffer.name.clone_from(name);
                }

                buffer.arguments.push_str(arguments_delta);

                Vec::new()
            }

            IrEvent::ToolCallDone {
                index,
                call_id,
                name,
                arguments,
                ..
            } => {
                let buffered = self
                    .tool_buffers
                    .iter()
                    .find(|buffer| buffer.ir_index == *index)
                    .map(|buffer| buffer.arguments.clone())
                    .unwrap_or_default();

                // A done carrying no assembled arguments falls back to the streamed bytes,
                // mirroring the aggregation rule.
                let full_arguments = if arguments.is_empty() {
                    buffered
                } else {
                    arguments.clone()
                };

                let mut frames = self.close_open_block();
                let block_index = self.next_index;

                self.next_index += 1;

                frames.push(Self::frame(
                    "content_block_start",
                    &ContentBlockStartPayload {
                        kind: "content_block_start",
                        index: block_index,
                        content_block: StartBlockOut::ToolUse {
                            id: call_id.clone(),
                            name: name.clone(),
                            input: crate::format::EmptyObject {},
                        },
                    },
                ));

                frames.push(Self::frame(
                    "content_block_delta",
                    &ContentBlockDeltaPayload {
                        kind: "content_block_delta",
                        index: block_index,
                        delta: DeltaOut::InputJson {
                            partial_json: full_arguments,
                        },
                    },
                ));

                frames.push(Self::frame(
                    "content_block_stop",
                    &ContentBlockStopPayload {
                        kind: "content_block_stop",
                        index: block_index,
                    },
                ));

                self.tool_buffers.retain(|buffer| buffer.ir_index != *index);

                frames
            }

            IrEvent::Finish { reason, .. } => {
                self.finish = Some(*reason);

                // No terminal delta here: cross-format upstreams deliver usage after the
                // finish reason, and the delta must carry the final report — Done renders it.
                self.close_open_block()
            }

            IrEvent::CompactionMetadata {
                is_paused,
                iterations,
            } => {
                if *is_paused {
                    self.forced_stop_reason = Some(String::from("compaction"));
                }
                self.native_iterations = iterations.clone();
                Vec::new()
            }
            IrEvent::Usage(usage) => {
                self.usage = Some(*usage);

                Vec::new()
            }

            IrEvent::Done => {
                let mut frames = self.close_open_block();

                // The terminal delta rides Done, where usage is final. A clean end-of-stream
                // without a finish degrades to `end_turn`, so streamed and aggregated
                // renderings of the same stream agree.
                frames.push(self.message_delta_frame(self.finish.unwrap_or(IrFinishReason::Stop)));

                frames.push(Self::frame(
                    "message_stop",
                    &MessageStopPayload {
                        kind: "message_stop",
                    },
                ));

                frames
            }
        });

        frames
    }

    /// Renders a mid-stream error as a terminal `error` event, classified once through the
    /// shared API error (README §6.7). A failure while the `message_start` head is still
    /// buffered flushes it ahead of the error frame — exactly once and first — so a stream
    /// that fails between the start and its first resolving event still opens natively; the
    /// two frames ride one body write, which SSE clients split on the blank-line delimiter.
    fn render_error_frame(&mut self, error: &crate::api::Error) -> String {
        let mut head = String::new();

        if self.is_start_buffered {
            self.is_start_buffered = false;

            head.push_str(&self.message_start_frame(ResponseUsage::zero()));
            head.extend(self.drain_compaction_prefix());
        }

        head + &Self::frame(
            "error",
            &StreamErrorPayload {
                kind: "error",
                error: render_body(error),
            },
        )
    }
}

impl ClientSseRenderer for SseRenderer {
    fn take_completed_artifact(&mut self) -> Option<crate::format::CompletedArtifact> {
        self.completed_artifact.take()
    }

    fn take_prefix_boundary(&mut self) -> Option<usize> {
        self.prefix_boundary.take()
    }

    fn begin_projection(&mut self, projector: crate::affinity::fingerprint::Projector) {
        self.projection = Some(super::affinity::Output::new(projector));
    }

    fn begin_checkpoint_projection(&mut self, projector: crate::affinity::fingerprint::Projector) {
        self.begin_projection(projector);
        self.is_prefix_prepared = true;
    }

    fn take_projection(&mut self) -> Option<crate::affinity::fingerprint::Projector> {
        self.projection
            .take()
            .and_then(super::affinity::Output::finish)
    }

    fn render(&mut self, event: &IrEvent) -> Vec<String> {
        self.completed_artifact = None;
        let frames = self.render_frames(event);
        if let Some(projection) = &mut self.projection {
            let mut is_completed_state = false;
            if let IrEvent::CompactionState { payload } = event {
                if super::compaction_affinity::is_native_no_op(payload) {
                    is_completed_state = true;
                } else if let Some((window, anchors)) =
                    super::compaction_affinity::completed_projection(payload, None)
                {
                    projection.replace_window(window);
                    // A complete native compaction always renders start/stop frames here.
                    self.completed_artifact = Some(crate::format::CompletedArtifact {
                        terminal_index: frames.len() - 1,
                        anchors,
                    });
                    is_completed_state = true;
                }
            }
            if !is_completed_state {
                let first = if self.is_prefix_prepared {
                    self.prefix_boundary.map_or(0, |index| index + 1)
                } else {
                    0
                };
                #[expect(
                    clippy::indexing_slicing,
                    reason = "the prefix boundary belongs to this batch"
                )]
                projection.observe(event, &frames[first..]);
            }
        }
        frames
    }

    fn error_frame(&mut self, error: &crate::api::Error) -> String {
        self.render_error_frame(error)
    }
}

/// Projects actual output blocks through the same accepted assistant-block conversion as replay.
pub(super) fn project_json_response(
    response: &MessagesResponse,
    projector: &mut crate::affinity::fingerprint::Projector,
) -> bool {
    project_json_after_prefix(response, projector, 0)
}

/// Projects actual answer fields after the already normalized prepared checkpoint prefix.
pub(super) fn project_json_after_prefix(
    response: &MessagesResponse,
    projector: &mut crate::affinity::fingerprint::Projector,
    prefix_count: usize,
) -> bool {
    response
        .content
        .iter()
        .skip(prefix_count)
        .all(|block| super::json_affinity::project_block(block, projector))
}

/// Identifies actual replayable native blocks without inferring gateway provenance or origin.
pub(super) fn completed_json_artifacts(
    response: &MessagesResponse,
) -> Vec<crate::affinity::Anchor> {
    response
        .content
        .iter()
        .flat_map(|block| match block {
            OutBlock::Verbatim(value) => {
                super::compaction_affinity::completed_projection(value, None)
                    .map_or_else(Vec::new, |(_, anchors)| anchors)
            }
            OutBlock::Checkpoint(checkpoint) => {
                super::compaction_affinity::completed_anchors(checkpoint, None)
            }
            OutBlock::Typed(_) => Vec::new(),
        })
        .collect()
}
