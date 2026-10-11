//! Intermediate representation shared by every protocol translation.
//!
//! Client requests parse into an [`IrRequest`]; every upstream response — streamed or buffered —
//! parses into a sequence of [`IrEvent`] values; every client response renders from IR. The
//! compaction workflows' iteration reports and native control entries — the IR's compaction
//! facts — live in the [`compaction`] submodule.

use crate::vocabulary::ReasoningLevel;
use serde::{Deserialize, Serialize};
use serde_json::{Error as JsonError, Value};

pub(crate) mod compaction;

/// A client request failed validation: the request is malformed, carries an unsupported value, or
/// names an unknown field.
///
/// One type shared by every request validator — the format codecs and the IR parsing helpers —
/// so each HTTP surface renders it in its own contract. It never carries a status or wire code:
/// those belong to the surfaces.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(crate) struct InvalidRequest {
    /// Offending request parameter, when the rejection names one.
    pub(crate) param: Option<String>,

    /// Human-readable rejection message.
    message: String,
}

impl InvalidRequest {
    /// Builds a rejection of the request as a whole.
    pub(crate) fn request(message: impl Into<String>) -> Self {
        Self {
            param: None,
            message: message.into(),
        }
    }

    /// Builds a rejection naming the offending parameter.
    pub(crate) fn field(param: &str, message: impl Into<String>) -> Self {
        Self {
            param: Some(param.to_string()),
            message: message.into(),
        }
    }

    /// Builds a rejection from a body that is not valid JSON.
    pub(crate) fn from_json(error: JsonError) -> Self {
        Self::request(format!("Invalid JSON body: {error}"))
    }
}

impl ReasoningLevel {
    /// Parses the closed vocabulary string for a request parameter (`docs/reference/inference.md`):
    /// the request boundary's rejection of a foreign level, naming `param` in its own error
    /// domain.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRequest`] naming `param` for unknown strings.
    pub(crate) fn parse(value: &str, param: &str) -> Result<Self, InvalidRequest> {
        Self::from_vocabulary_str(value).ok_or_else(|| {
            InvalidRequest::field(param, format!("Unknown reasoning effort `{value}`"))
        })
    }
}

/// One assistant reasoning block: its text (when the family exposes text) plus the opaque replay
/// payload — enough to represent both an Anthropic-style `thinking` block and a
/// `redacted_thinking` block, and an Interactions `thought` step, verbatim
/// (`docs/reference/inference.md`).
///
/// Same-family round-trip is mandatory: Anthropic inbound thinking/redacted blocks and
/// Interactions inbound thought steps replay byte-exact to their own upstreams. OpenAI surfaces
/// render the block's text through the existing reasoning channels and drop the opaque payload.
/// Cross-family, the opaque payload is dropped (signatures are family-bound).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IrReasoningBlock {
    /// Reasoning text, when the family exposes text.
    pub(crate) text: String,

    /// Upstream signature terminating the block, when the family exposes one.
    pub(crate) signature: Option<String>,

    /// Opaque family-bound replay payload, stored verbatim from the inbound block JSON.
    /// [`Value::Null`] when the block carries no replay payload.
    pub(crate) opaque: Value,
}

impl IrReasoningBlock {
    /// Builds a text-only block with no replay payload.
    fn text_only(text: String) -> Self {
        Self {
            text,
            signature: None,
            opaque: Value::Null,
        }
    }
}

/// Reason a completion finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IrFinishReason {
    /// The model completed its output.
    Stop,

    /// The output hit a token limit.
    Length,

    /// The model requested tool execution.
    ToolCall,

    /// The output was cut off by a content filter.
    ContentFilter,
}

/// Role of a conversation participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IrRole {
    /// The human side of the conversation.
    User,

    /// The model side of the conversation.
    Assistant,

    /// A tool result delivered back to the model.
    Tool,
}

/// Image content accepted from clients and rendered to upstreams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum IrImage {
    /// Base64-encoded inline image data with its MIME type.
    Inline {
        /// MIME type of the image data.
        mime_type: String,

        /// Base64-encoded image bytes.
        data_base64: String,
    },

    /// Remote image referenced by URL.
    Url(String),
}

impl IrImage {
    /// Parses a client-supplied image reference: a `data:<mime>;base64,<data>` URL becomes inline
    /// data, an `http`/`https` URL stays remote.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRequest`] for schemes other than `data:`, `http:`, and `https:`, and for
    /// malformed data URLs.
    pub(crate) fn from_url(url: &str) -> Result<Self, InvalidRequest> {
        if let Some(rest) = url.strip_prefix("data:") {
            let (metadata, data) = rest.split_once(',').ok_or_else(|| {
                InvalidRequest::field("image_url", "Data URL is missing its payload")
            })?;

            let mime_type = metadata.strip_suffix(";base64").ok_or_else(|| {
                InvalidRequest::field("image_url", "Only base64-encoded data URLs are supported")
            })?;

            return Ok(Self::Inline {
                mime_type: mime_type.to_string(),
                data_base64: data.to_string(),
            });
        }

        if url.starts_with("http://") || url.starts_with("https://") {
            return Ok(Self::Url(url.to_string()));
        }

        Err(InvalidRequest::field(
            "image_url",
            "Image URLs must use the data:, http:, or https: schemes",
        ))
    }
}

/// One typed segment of a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum IrPart {
    /// Plain text content.
    Text {
        /// The text content.
        text: String,
    },

    /// Image content.
    Image {
        /// The image payload.
        image: IrImage,
    },

    /// An assistant tool invocation.
    ToolCall {
        /// Identifier of the invocation, round-tripped to the matching tool output.
        call_id: String,

        /// Name of the invoked function.
        name: String,

        /// JSON-encoded arguments object.
        arguments: String,

        /// Upstream thought signature that must be reattached across turns.
        thought_signature: Option<String>,
    },

    /// A tool result for a prior [`IrPart::ToolCall`].
    ToolOutput {
        /// Identifier of the invocation being answered.
        call_id: String,

        /// JSON or plain-text result content.
        content: String,

        /// Whether the tool reported an error; read and rendered by the Anthropic surface pair
        /// only, dropped by every other surface (`docs/reference/inference.md`).
        is_error: bool,
    },

    /// An assistant reasoning block (`docs/reference/inference.md`): its text plus the opaque
    /// family-bound replay payload. Same-family round-trip replays the opaque payload verbatim;
    /// OpenAI surfaces render the text through the existing reasoning channels and drop the
    /// payload.
    Reasoning {
        /// Reasoning text, when the family exposes text.
        text: String,

        /// Upstream signature terminating the block, when the family exposes one.
        signature: Option<String>,

        /// Opaque family-bound replay payload, stored verbatim from the inbound block JSON.
        opaque: Value,
    },
}

/// One conversation message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IrMessage {
    /// Participating role.
    pub(crate) role: IrRole,

    /// Ordered typed segments.
    pub(crate) parts: Vec<IrPart>,
}

/// A function tool advertised to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IrTool {
    /// Function name the model calls.
    pub(crate) name: String,

    /// Human-readable description of the function.
    pub(crate) description: Option<String>,

    /// JSON Schema of the accepted arguments.
    pub(crate) parameters: Option<Value>,
}

/// Restriction on which tool the model may call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IrToolChoice {
    /// The model decides freely.
    Auto,

    /// The model may not call tools.
    None,

    /// The model must call a tool.
    Required,

    /// The model must call the named function.
    Function {
        /// Required function name.
        name: String,
    },
}

/// Response format restriction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IrResponseFormat {
    /// Free-form text (the implicit default).
    Text,

    /// Any valid JSON value.
    Json,

    /// JSON conforming to a named schema.
    JsonSchema {
        /// Schema name.
        name: String,

        /// JSON Schema of the response.
        schema: Value,
    },
}

/// The producer family of a state item: the protocol whose upstream renders
/// the item verbatim (`docs/reference/inference.md`). Only the two protocols with a compaction
/// state channel produce state items; every other surface rejects them all,
/// so the family has no further variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StateFamily {
    /// OpenAI Responses: a `compaction` input item or a replayed reasoning
    /// item carrying `encrypted_content`.
    Responses,

    /// Anthropic Messages: a `compaction` content block.
    Messages,
}

/// A vendor-shaped state item riding the request: opaque native compaction
/// state or a gateway carrier recognized in the inbound history, preserved at
/// its history position.
///
/// The upstream render of the item's own family splices it into the request at
/// its offset; every other upstream render rejects its presence, because
/// opaque native state another protocol cannot consume is never silently
/// discarded. Gateway carriers (`is_gateway`) never reach an upstream at all:
/// they are decoded and removed before rendering, and their presence here
/// after preparation is an internal invariant violation.
///
/// The two carried facts are orthogonal and neither is re-derived from the
/// item's wire shape:
///
/// - `family` records which producer family's wire the item speaks. The parsing codecs stamp their
///   own protocol — for a gateway carrier too, because a carrier is protocol-shaped, minted in the
///   client protocol's own carrier form — and the internal producers stamp the wrapped native
///   window's family, the remap filters carrying the fact through unchanged.
/// - `is_gateway` marks the item a sealed carrier instead of native state. A render therefore
///   accepts a state item only when it is native and its family is the render's own; a gateway item
///   fails as the preparation invariant violation, a foreign-family item as inaccessible state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StateItem {
    /// Number of IR messages rendered before this item in the upstream
    /// request.
    pub(crate) offset: usize,

    /// The vendor-shaped item, verbatim.
    pub(crate) item: Value,

    /// Whether the item is a recognized gateway carrier rather than native
    /// state.
    pub(crate) is_gateway: bool,

    /// Whether the item is compaction state — a native compaction block or
    /// item, or a gateway carrier over one; every other opaque item, such as
    /// replayable encrypted reasoning, is not. The i-th compaction-typed state
    /// item is the i-th parsed compaction boundary, both pushed in encounter
    /// order by the codecs, so boundary preparation pairs them by this carried
    /// fact instead of re-parsing the vendor JSON.
    pub(crate) is_compaction: bool,

    /// The producer family whose upstream renders the item verbatim (see
    /// [`StateFamily`]).
    pub(crate) family: StateFamily,
}

/// A fully format-independent completion request.
#[derive(Debug, Clone, Default)]
pub(crate) struct IrRequest {
    /// Whether a selected native Responses trigger operation appends its terminal control item.
    pub(crate) has_compaction_trigger: bool,

    /// Upstream model name rendered verbatim into upstream payloads.
    pub(crate) model: String,

    /// Concatenated system instructions.
    pub(crate) instructions: Option<String>,

    /// Conversation messages in order.
    pub(crate) messages: Vec<IrMessage>,

    /// Available function tools.
    pub(crate) tools: Vec<IrTool>,

    /// Tool selection restriction.
    pub(crate) tool_choice: Option<IrToolChoice>,

    /// Sampling temperature.
    pub(crate) temperature: Option<f64>,

    /// Nucleus sampling threshold.
    pub(crate) top_p: Option<f64>,

    /// Output token budget.
    pub(crate) max_output_tokens: Option<u64>,

    /// Response format restriction.
    pub(crate) response_format: Option<IrResponseFormat>,

    /// Reasoning effort level in the closed shared vocabulary (`docs/reference/inference.md`):
    /// parsed from the client's own parameter into [`ReasoningLevel`], written verbatim into
    /// the upstream's own parameter. `None` means the client omitted the field, and then no
    /// upstream parameter is rendered.
    pub(crate) reasoning_effort: Option<ReasoningLevel>,

    /// Opaque state items recognized in the history, each preserved at its
    /// position (see [`StateItem`]). Empty for requests carrying none.
    pub(crate) state_items: Vec<StateItem>,
}

/// One event of a completion, in upstream-agnostic form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IrEvent {
    /// Identifies the completion; emitted exactly once, first.
    MessageStart {
        /// Upstream response identifier.
        id: String,

        /// Upstream model name.
        model: String,
    },

    /// Incremental assistant text.
    TextDelta(String),

    /// Incremental reasoning or summary text.
    ReasoningDelta(String),

    /// A complete reasoning block, terminating the reasoning text streamed so far with its
    /// signature so streaming and aggregated renderings agree (`docs/reference/inference.md`).
    /// Upstream parsers emit either deltas plus an empty-text done, or a done carrying the full
    /// text with no prior deltas — never both with overlapping text — so renderers can render
    /// both safely: a non-empty `text` renders when no deltas preceded it, an empty `text`
    /// closes the block the deltas already delivered.
    ReasoningDone {
        /// Complete reasoning text of the block; empty when streamed deltas delivered it.
        text: String,

        /// Upstream signature terminating the block.
        signature: Option<String>,

        /// Opaque family-bound replay payload.
        opaque: Value,
    },

    /// Incremental tool call assembly; `index` is stable per call.
    ToolCallDelta {
        /// Stable index of the call within the response.
        index: usize,

        /// Identifier of the invocation, when first revealed.
        call_id: Option<String>,

        /// Name of the invoked function, when first revealed.
        name: Option<String>,

        /// Incremental JSON-encoded arguments text.
        arguments_delta: String,
    },

    /// A complete tool call.
    ToolCallDone {
        /// Stable index of the call within the response.
        index: usize,

        /// Identifier of the invocation.
        call_id: String,

        /// Name of the invoked function.
        name: String,

        /// Complete JSON-encoded arguments text.
        arguments: String,

        /// Upstream thought signature that must be reattached across turns.
        thought_signature: Option<String>,
    },

    /// The completion finished.
    Finish {
        /// Reason the completion finished.
        reason: IrFinishReason,

        /// Verbatim upstream prompt-block reason, set exactly when the finish is the
        /// content-filter finish a parser derived from its upstream's prompt-feedback
        /// channel; every other finish carries [`None`]. Family-bound round-trip detail in
        /// the [`IrPart::ToolCall`] `thought_signature` mold: the native GenerateContent
        /// surface renders it as `promptFeedback.blockReason` (`docs/reference/inference.md`),
        /// every other surface ignores it.
        prompt_block_reason: Option<String>,
    },

    /// A native compaction state item observed in the upstream output, verbatim:
    /// a Responses `compaction` output item or an Anthropic compaction content
    /// block. Parsers emit it at the item's arrival position so the outward
    /// render preserves every distinct native compaction and its position;
    /// renderers emit it in the client's protocol shape. Never emitted for
    /// gateway carriers, which are decoded before any upstream dispatch.
    CompactionState {
        /// The vendor-shaped item or block, verbatim.
        payload: Value,
    },

    /// Native compaction lifecycle metadata, independent of ordinary generation usage.
    CompactionMetadata {
        /// Whether the upstream paused after compaction.
        is_paused: bool,
        /// Vendor iteration reports, preserved verbatim.
        iterations: Vec<compaction::UsageIteration>,
    },
    /// Token accounting.
    Usage(IrUsage),

    /// Terminal marker; always appended by the gateway after the upstream response is fully
    /// consumed, never by parsers.
    Done,
}

impl IrEvent {
    /// Builds a plain [`IrEvent::Finish`] carrying no prompt-block detail — the shape of
    /// every finish but a prompt-level block.
    pub(crate) fn finish(reason: IrFinishReason) -> Self {
        Self::Finish {
            reason,
            prompt_block_reason: None,
        }
    }
}

/// Token accounting of a completion.
///
/// The breakdown values mirror the official usage detail objects: `cached_tokens` and
/// `cache_write_tokens` detail the input, and `reasoning_tokens` details the output. They are
/// [`None`] when the upstream reports no breakdown — an upstream that reports totals only, such
/// as a Responses upstream without detail objects or a Chat Completions upstream that omits its
/// `prompt_tokens_details`/`completion_tokens_details` objects — so the outward Responses
/// rendering can zero-fill the SDK-required detail fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct IrUsage {
    /// Tokens consumed by the input.
    pub(crate) input_tokens: u64,

    /// Tokens produced by the output.
    pub(crate) output_tokens: u64,

    /// Input tokens served from the upstream prompt cache.
    pub(crate) cached_tokens: Option<u64>,

    /// Input tokens written to the upstream prompt cache.
    pub(crate) cache_write_tokens: Option<u64>,

    /// Output tokens spent on model reasoning.
    pub(crate) reasoning_tokens: Option<u64>,
}

impl IrUsage {
    /// Sums two observations field by field, saturating rather than
    /// overflowing; a breakdown column sums when either observation reported
    /// it, so a reported breakdown is never lost to an unreporting sibling.
    #[must_use]
    pub(crate) fn saturating_sum(self, other: Self) -> Self {
        let sum_option = |left: Option<u64>, right: Option<u64>| match (left, right) {
            (Some(left), Some(right)) => Some(left.saturating_add(right)),
            (left, right) => left.or(right),
        };

        Self {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            cached_tokens: sum_option(self.cached_tokens, other.cached_tokens),
            cache_write_tokens: sum_option(self.cache_write_tokens, other.cache_write_tokens),
            reasoning_tokens: sum_option(self.reasoning_tokens, other.reasoning_tokens),
        }
    }
}

/// One completed tool call produced by aggregation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IrAggregatedToolCall {
    /// Identifier of the invocation.
    pub(crate) call_id: String,

    /// Name of the invoked function.
    pub(crate) name: String,

    /// Complete JSON-encoded arguments text.
    pub(crate) arguments: String,

    /// Upstream thought signature to reattach across turns.
    pub(crate) thought_signature: Option<String>,
}

/// The complete result of folding an IR event stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IrAggregation {
    /// All accumulated assistant text.
    pub(crate) text: String,

    /// All accumulated reasoning text.
    pub(crate) reasoning: String,

    /// Completed reasoning blocks in emission order, carrying their signatures and opaque replay
    /// payloads so streaming and aggregated renderings agree (`docs/reference/inference.md`).
    pub(crate) reasoning_blocks: Vec<IrReasoningBlock>,

    /// Completed tool calls in emission order.
    pub(crate) tool_calls: Vec<IrAggregatedToolCall>,

    /// Reason the completion finished, when reported.
    pub(crate) finish: Option<IrFinishReason>,

    /// Verbatim upstream prompt-block reason when the finish is a prompt-level block
    /// (`docs/reference/inference.md`); [`None`] for every other finish.
    pub(crate) prompt_block_reason: Option<String>,

    /// Token accounting, when reported.
    pub(crate) usage: Option<IrUsage>,

    /// Whether native compaction paused the response.
    pub(crate) is_compaction_paused: bool,
    /// Native sampling iteration reports, authoritative for billed usage.
    pub(crate) compaction_iterations: Vec<compaction::UsageIteration>,
    /// Native compaction state items in arrival order (see
    /// [`IrEvent::CompactionState`]). The aggregated renderers emit them ahead
    /// of the derived output items — a native compaction precedes the answer
    /// output it precedes upstream, and the arrival order of the state items
    /// themselves is preserved.
    pub(crate) compaction_states: Vec<Value>,
}

/// Folds IR events into an [`IrAggregation`].
#[derive(Debug, Default)]
pub(crate) struct Aggregator {
    /// State under construction.
    aggregation: IrAggregation,

    /// Arguments accumulated per in-flight tool call index; consulted when a
    /// [`IrEvent::ToolCallDone`] arrives without assembled `arguments` of its own.
    pending_arguments: Vec<(usize, String)>,

    /// Tool call indexes already completed in this aggregation; a repeated done for one of them is
    /// ignored.
    completed_call_indexes: Vec<usize>,

    /// Reasoning text streamed since the last [`IrEvent::ReasoningDone`]; consulted when a done
    /// arrives without its own complete `text`.
    pending_reasoning: String,
}

impl Aggregator {
    /// Ingests one event, updating the aggregation in place.
    pub(crate) fn ingest(&mut self, event: &IrEvent) {
        match event {
            // MessageStart carries the upstream identity only as the streaming renderers' start
            // signal; aggregation records nothing from it — the outward responses carry
            // renderer-minted identifiers.
            IrEvent::MessageStart { .. } | IrEvent::Done => {}

            IrEvent::TextDelta(delta) => self.aggregation.text.push_str(delta),

            IrEvent::ReasoningDelta(delta) => {
                self.aggregation.reasoning.push_str(delta);
                self.pending_reasoning.push_str(delta);
            }

            IrEvent::ReasoningDone {
                text,
                signature,
                opaque,
            } => {
                // The done carries the block's complete text; when it arrives empty, the streamed
                // deltas since the previous done are the authoritative text — mirroring the
                // ToolCallDone fallback — so a delta-only upstream and a done-only upstream fold
                // to the same block. When it carries text, that text is authoritative: the pending
                // deltas' bytes already sit at the tail of `reasoning`, so they are truncated off
                // before the authoritative text is pushed.
                if text.is_empty() {
                    let block_text = std::mem::take(&mut self.pending_reasoning);

                    self.aggregation.reasoning_blocks.push(IrReasoningBlock {
                        text: block_text,
                        signature: signature.clone(),
                        opaque: opaque.clone(),
                    });
                } else {
                    let pending = std::mem::take(&mut self.pending_reasoning);

                    self.aggregation.reasoning.truncate(
                        self.aggregation
                            .reasoning
                            .len()
                            .saturating_sub(pending.len()),
                    );
                    self.aggregation.reasoning.push_str(text);

                    self.aggregation.reasoning_blocks.push(IrReasoningBlock {
                        text: text.clone(),
                        signature: signature.clone(),
                        opaque: opaque.clone(),
                    });
                }
            }

            IrEvent::ToolCallDelta {
                index,
                arguments_delta,
                ..
            } => {
                match self
                    .pending_arguments
                    .iter_mut()
                    .find(|(pending_index, _)| pending_index == index)
                {
                    Some(pending) => pending.1.push_str(arguments_delta),

                    None => self
                        .pending_arguments
                        .push((*index, arguments_delta.clone())),
                }
            }

            IrEvent::ToolCallDone {
                index,
                call_id,
                name,
                arguments,
                thought_signature,
            } => {
                // Conformant streams complete each call index exactly once; a repeated done for an
                // already-completed index is ignored, so a duplicating upstream cannot
                // double-deliver a call.
                if !self.completed_call_indexes.contains(index) {
                    self.completed_call_indexes.push(*index);

                    let accumulated = self.take_pending_arguments(*index);

                    // Upstreams normally assemble the complete arguments in the done event itself;
                    // when one arrives without them, the streamed deltas are the authoritative
                    // arguments.
                    let arguments = if arguments.is_empty() {
                        accumulated
                    } else {
                        arguments.clone()
                    };

                    self.aggregation.tool_calls.push(IrAggregatedToolCall {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments,
                        thought_signature: thought_signature.clone(),
                    });
                }
            }

            IrEvent::Finish {
                reason,
                prompt_block_reason,
            } => {
                self.aggregation.finish = Some(*reason);
                self.aggregation.prompt_block_reason = prompt_block_reason.clone();
            }

            IrEvent::CompactionState { payload } => {
                self.aggregation.compaction_states.push(payload.clone());
            }

            IrEvent::CompactionMetadata {
                is_paused,
                iterations,
            } => {
                self.aggregation.is_compaction_paused = *is_paused;
                self.aggregation.compaction_iterations = iterations.clone();
            }
            IrEvent::Usage(usage) => self.aggregation.usage = Some(*usage),
        }
    }

    /// Consumes the accumulator, flushing an unterminated reasoning span as a text-only block so
    /// a delta-only stream without a closing done still yields its block for aggregated
    /// renderings; unfinished in-flight tool call state is dropped.
    pub(crate) fn finish(mut self) -> IrAggregation {
        if !self.pending_reasoning.is_empty() {
            let text = std::mem::take(&mut self.pending_reasoning);

            self.aggregation
                .reasoning_blocks
                .push(IrReasoningBlock::text_only(text));
        }

        self.aggregation
    }

    /// Removes and returns the arguments accumulated for one in-flight tool call index.
    fn take_pending_arguments(&mut self, index: usize) -> String {
        match self
            .pending_arguments
            .iter()
            .position(|(pending_index, _)| *pending_index == index)
        {
            Some(position) => self.pending_arguments.swap_remove(position).1,

            None => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the IR's own decisions: the aggregation fold rules over IR event streams, the
    //! saturating usage composition, the accepted image URL forms, and the request-boundary
    //! reasoning-level rejection (`docs/reference/inference.md`).

    #![expect(
        clippy::missing_panics_doc,
        clippy::panic,
        clippy::expect_used,
        reason = "a test fails by panicking; a # Panics section on every test would restate that"
    )]

    use super::{
        Aggregator, InvalidRequest, IrAggregatedToolCall, IrAggregation, IrEvent, IrFinishReason,
        IrImage, IrPart, IrReasoningBlock, IrRole, IrUsage,
    };
    use crate::vocabulary::ReasoningLevel;
    use serde_json::json;

    /// Folds one event stream through an aggregator and returns the result.
    fn folded(events: &[IrEvent]) -> IrAggregation {
        let mut aggregator = Aggregator::default();

        for event in events {
            aggregator.ingest(event);
        }

        aggregator.finish()
    }

    /// Text and reasoning deltas accumulate into their own spans; a reasoning done carrying its
    /// own text is authoritative over pending deltas, while an empty-text done closes the
    /// streamed deltas as the block.
    #[test]
    fn text_and_reasoning_folds_follow_authoritative_dones() {
        let aggregation = folded(&[
            IrEvent::MessageStart {
                id: "resp-1".to_string(),
                model: "m".to_string(),
            },
            IrEvent::TextDelta("Hello ".to_string()),
            IrEvent::TextDelta("world".to_string()),
            IrEvent::ReasoningDelta("think".to_string()),
            IrEvent::ReasoningDelta("ing".to_string()),
            IrEvent::ReasoningDone {
                text: String::new(),
                signature: Some("sig".to_string()),
                opaque: json!({"i": 1}),
            },
            IrEvent::ReasoningDelta("second".to_string()),
            IrEvent::ReasoningDone {
                text: "authoritative".to_string(),
                signature: None,
                opaque: json!(null),
            },
            IrEvent::TextDelta("!".to_string()),
            IrEvent::Done,
        ]);

        assert_eq!(aggregation.text, "Hello world!");
        assert_eq!(aggregation.reasoning, "thinkingauthoritative");
        assert_eq!(
            aggregation.reasoning_blocks,
            vec![
                IrReasoningBlock {
                    text: "thinking".to_string(),
                    signature: Some("sig".to_string()),
                    opaque: json!({"i": 1}),
                },
                IrReasoningBlock {
                    text: "authoritative".to_string(),
                    signature: None,
                    opaque: json!(null),
                },
            ]
        );
        assert_eq!(aggregation.finish, None);
    }

    /// A delta-only stream without a closing done still yields its unterminated reasoning span
    /// as a text-only block, so aggregated renderings never lose a reasoning prefix.
    #[test]
    fn unterminated_reasoning_flushes_as_a_text_only_block() {
        let aggregation = folded(&[
            IrEvent::ReasoningDelta("partial".to_string()),
            IrEvent::TextDelta("answer".to_string()),
        ]);

        assert_eq!(aggregation.text, "answer");
        assert_eq!(aggregation.reasoning, "partial");
        assert_eq!(
            aggregation.reasoning_blocks,
            vec![IrReasoningBlock {
                text: "partial".to_string(),
                signature: None,
                opaque: json!(null),
            }]
        );
    }

    /// Tool call arguments assemble per index: deltas accumulate under their stable index, a
    /// done without arguments uses the accumulated text, a done with arguments is
    /// authoritative, and a repeated done for one index is ignored so a duplicating upstream
    /// cannot double-deliver a call.
    #[test]
    fn tool_calls_assemble_per_index_and_ignore_repeated_dones() {
        let aggregation = folded(&[
            IrEvent::ToolCallDelta {
                index: 0,
                call_id: None,
                name: None,
                arguments_delta: "{\"a\"".to_string(),
            },
            IrEvent::ToolCallDelta {
                index: 1,
                call_id: Some("call-2".to_string()),
                name: Some("g".to_string()),
                arguments_delta: "[1,".to_string(),
            },
            IrEvent::ToolCallDelta {
                index: 0,
                call_id: Some("call-1".to_string()),
                name: Some("f".to_string()),
                arguments_delta: ": 1}".to_string(),
            },
            IrEvent::ToolCallDelta {
                index: 1,
                call_id: None,
                name: None,
                arguments_delta: "2]".to_string(),
            },
            IrEvent::ToolCallDone {
                index: 0,
                call_id: "call-1".to_string(),
                name: "f".to_string(),
                arguments: String::new(),
                thought_signature: Some("sig".to_string()),
            },
            IrEvent::ToolCallDone {
                index: 1,
                call_id: "call-2".to_string(),
                name: "g".to_string(),
                arguments: "[9]".to_string(),
                thought_signature: None,
            },
            IrEvent::ToolCallDone {
                index: 0,
                call_id: "call-1".to_string(),
                name: "f".to_string(),
                arguments: "{\"a\": 2}".to_string(),
                thought_signature: None,
            },
        ]);

        assert_eq!(
            aggregation.tool_calls,
            vec![
                IrAggregatedToolCall {
                    call_id: "call-1".to_string(),
                    name: "f".to_string(),
                    arguments: "{\"a\": 1}".to_string(),
                    thought_signature: Some("sig".to_string()),
                },
                IrAggregatedToolCall {
                    call_id: "call-2".to_string(),
                    name: "g".to_string(),
                    arguments: "[9]".to_string(),
                    thought_signature: None,
                },
            ]
        );
    }

    /// The finish, compaction, and usage events fold into their aggregation fields while the
    /// start and terminal markers record nothing, and the plain-finish builder carries no
    /// prompt-block detail.
    #[test]
    fn terminal_and_compaction_events_fold_into_their_fields() {
        let mut aggregator = Aggregator::default();

        aggregator.ingest(&IrEvent::TextDelta("t".to_string()));
        aggregator.ingest(&IrEvent::CompactionState {
            payload: json!({"window": 1}),
        });
        aggregator.ingest(&IrEvent::CompactionState {
            payload: json!({"window": 2}),
        });
        aggregator.ingest(&IrEvent::CompactionMetadata {
            is_paused: true,
            iterations: Vec::new(),
        });
        aggregator.ingest(&IrEvent::Usage(IrUsage {
            input_tokens: 3,
            ..IrUsage::default()
        }));
        aggregator.ingest(&IrEvent::finish(IrFinishReason::ToolCall));

        let aggregation = aggregator.finish();

        assert_eq!(aggregation.finish, Some(IrFinishReason::ToolCall));
        assert_eq!(aggregation.prompt_block_reason, None);
        assert!(aggregation.is_compaction_paused);
        assert_eq!(
            aggregation.compaction_states,
            vec![json!({"window": 1}), json!({"window": 2})]
        );
        assert_eq!(
            aggregation.usage,
            Some(IrUsage {
                input_tokens: 3,
                ..IrUsage::default()
            })
        );

        let blocked = folded(&[IrEvent::Finish {
            reason: IrFinishReason::ContentFilter,
            prompt_block_reason: Some("SAFETY".to_string()),
        }]);

        assert_eq!(blocked.finish, Some(IrFinishReason::ContentFilter));
        assert_eq!(blocked.prompt_block_reason.as_deref(), Some("SAFETY"));
    }

    /// Usage sums saturate rather than overflow, and a reported breakdown column sums when both
    /// observations report it and is never lost to an unreporting sibling.
    #[test]
    fn usage_sums_saturate_and_keep_reported_breakdowns() {
        let reported = IrUsage {
            input_tokens: 10,
            output_tokens: u64::MAX,
            cached_tokens: Some(4),
            reasoning_tokens: Some(6),
            ..IrUsage::default()
        };
        let unreporting = IrUsage {
            input_tokens: 1,
            output_tokens: 2,
            cache_write_tokens: Some(7),
            ..IrUsage::default()
        };

        let summed = reported.saturating_sum(unreporting);

        assert_eq!(summed.input_tokens, 11);
        assert_eq!(summed.output_tokens, u64::MAX);
        assert_eq!(summed.cached_tokens, Some(4));
        assert_eq!(summed.cache_write_tokens, Some(7));
        assert_eq!(summed.reasoning_tokens, Some(6));

        let both_reporting = IrUsage {
            cached_tokens: Some(2),
            ..IrUsage::default()
        }
        .saturating_sum(IrUsage {
            cached_tokens: Some(3),
            ..IrUsage::default()
        });

        assert_eq!(both_reporting.cached_tokens, Some(5));
        assert_eq!(
            IrUsage::default().saturating_sum(IrUsage::default()),
            IrUsage::default()
        );
    }

    /// An image reference accepts `data:` URLs as inline base64 with their MIME type, keeps
    /// `http`/`https` remote, and rejects every other scheme and malformed data URL.
    #[test]
    fn image_references_accept_data_and_web_schemes_only() {
        let inline = IrImage::from_url("data:image/png;base64,aGVsbG8=").expect("a data URL");

        assert_eq!(
            inline,
            IrImage::Inline {
                mime_type: "image/png".to_string(),
                data_base64: "aGVsbG8=".to_string(),
            }
        );

        assert_eq!(
            IrImage::from_url("https://example.test/a.png").expect("an https URL"),
            IrImage::Url("https://example.test/a.png".to_string())
        );
        assert_eq!(
            IrImage::from_url("http://example.test/a.png").expect("an http URL"),
            IrImage::Url("http://example.test/a.png".to_string())
        );

        for malformed in [
            "ftp://example.test/a.png",
            "data:image/png;base64",
            "data:image/png,aGVsbG8=",
            "data:",
            "",
        ] {
            let error = match IrImage::from_url(malformed) {
                Err(error) => error,
                Ok(_) => panic!("the image reference `{malformed}` is rejected"),
            };

            assert_eq!(error.param.as_deref(), Some("image_url"));
        }
    }

    /// The request-boundary reasoning parse accepts the closed vocabulary and rejects a foreign
    /// level naming the parameter with the offending value.
    #[test]
    fn reasoning_parse_rejects_foreign_levels_naming_the_parameter() {
        assert!(matches!(
            ReasoningLevel::parse("xhigh", "effort"),
            Ok(ReasoningLevel::XHigh)
        ));

        let error =
            ReasoningLevel::parse("ultra", "effort").expect_err("a foreign level is rejected");

        assert_eq!(error.param.as_deref(), Some("effort"));
        assert!(error.to_string().contains("ultra"));
    }

    /// The invalid-request builders state where a rejection names a parameter and where it
    /// names the request as a whole.
    #[test]
    fn invalid_request_builders_name_their_scope() {
        let field = InvalidRequest::field("top_p", "out of range");
        let request = InvalidRequest::request("bad body");

        assert_eq!(field.param.as_deref(), Some("top_p"));
        assert_eq!(field.to_string(), "out of range");
        assert_eq!(request.param, None);
        assert_eq!(request.to_string(), "bad body");
    }

    /// The typed parts and roles render their snake_case IR spellings, which the state-carrying
    /// surfaces rely on for vendored wire shapes.
    #[test]
    fn parts_render_their_typed_spelling() {
        let message = super::IrMessage {
            role: IrRole::Tool,
            parts: vec![IrPart::ToolOutput {
                call_id: "c".to_string(),
                content: "{}".to_string(),
                is_error: true,
            }],
        };

        assert_eq!(
            serde_json::to_value(&message).expect("IR messages serialize"),
            json!({
                "role": "tool",
                "parts": [{
                    "kind": "tool_output",
                    "call_id": "c",
                    "content": "{}",
                    "is_error": true,
                }],
            })
        );
    }
}
