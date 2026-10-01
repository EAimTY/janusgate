//! Intermediate representation shared by every protocol translation.
//!
//! Client requests parse into an [`IrRequest`]; every upstream response — streamed or buffered —
//! parses into a sequence of [`IrEvent`] values; every client response renders from IR.

use serde_json::{Error as JsonError, Value};

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

/// Closed reasoning vocabulary shared by every format (README §6.4): one enum, forever.
///
/// Inbound surfaces parse their own parameter into this enum; unknown strings are a 400 at the
/// gateway. Upstream renderers write the enum verbatim into the target's own level parameter.
/// There is no cross-vocabulary mapping, and there never will be — a value the target does not
/// support is rejected by the upstream and mirrored to the client per the existing error policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IrReasoningLevel {
    /// No reasoning; renders as the native off spelling per format (README §6.4).
    None,

    /// Minimal reasoning effort.
    Minimal,

    /// Low reasoning effort.
    Low,

    /// Medium reasoning effort.
    Medium,

    /// High reasoning effort.
    High,

    /// Extra-high reasoning effort.
    XHigh,

    /// Maximum reasoning effort.
    Max,
}

impl IrReasoningLevel {
    /// The one (level, wire-spelling) table of the closed vocabulary (README §6.4): the
    /// [`Self::parse`] and [`Self::as_str`] matches and every format surface's conformance test
    /// pin derive from it, so a spelling change is a single edit here.
    #[cfg(test)]
    pub(crate) const VOCABULARY: [(IrReasoningLevel, &'static str); 7] = [
        (Self::None, "none"),
        (Self::Minimal, "minimal"),
        (Self::Low, "low"),
        (Self::Medium, "medium"),
        (Self::High, "high"),
        (Self::XHigh, "xhigh"),
        (Self::Max, "max"),
    ];

    /// The vocabulary in canonical order — the storage and declaration order of a level set
    /// (README §3/§5.5): a stored or declared set is deduplicated into this order at write.
    pub(crate) const CANONICAL: [Self; 7] = [
        Self::None,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    /// Parses a vocabulary string domain-neutrally: [`Some`] for a member, [`None`] for a
    /// foreign string. The vocabulary is shared by the request boundary (which 400s on a
    /// foreign value through [`Self::parse`]), the discovery ingestion boundary (which skips a
    /// foreign level, README §5.5), and the management override boundary (which rejects one) —
    /// each naming the vocabulary in its own error domain.
    #[must_use]
    pub(crate) fn from_vocabulary_str(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::XHigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }

    /// Canonicalizes a set of levels: deduplicated, in [`Self::CANONICAL`] order. Both write
    /// paths that store a level set — discovery ingestion and the management override — pass
    /// through here, so a stored set is always the canonical form (README §3).
    #[must_use]
    pub(crate) fn canonicalize_levels(levels: impl IntoIterator<Item = Self>) -> Vec<Self> {
        let mut present = [false; 7];

        for level in levels {
            #[expect(
                clippy::indexing_slicing,
                reason = "the canonical index of a vocabulary member is within the table by \
                          construction"
            )]
            {
                present[Self::canonical_index(level)] = true;
            }
        }

        #[expect(
            clippy::indexing_slicing,
            reason = "every enumerated index of CANONICAL is within the table by construction"
        )]
        Self::CANONICAL
            .into_iter()
            .enumerate()
            .filter(|(index, _)| present[*index])
            .map(|(_, level)| level)
            .collect()
    }

    /// The level's index in [`Self::CANONICAL`].
    const fn canonical_index(level: Self) -> usize {
        match level {
            Self::None => 0,
            Self::Minimal => 1,
            Self::Low => 2,
            Self::Medium => 3,
            Self::High => 4,
            Self::XHigh => 5,
            Self::Max => 6,
        }
    }

    /// Parses the closed vocabulary string.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRequest`] naming `param` for unknown strings.
    pub(crate) fn parse(value: &str, param: &str) -> Result<Self, InvalidRequest> {
        Self::from_vocabulary_str(value).ok_or_else(|| {
            InvalidRequest::field(param, format!("Unknown reasoning effort `{value}`"))
        })
    }

    /// Renders the canonical vocabulary string, written verbatim into the target's own level
    /// parameter.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// One assistant reasoning block: its text (when the family exposes text) plus the opaque replay
/// payload — enough to represent both an Anthropic-style `thinking` block and a
/// `redacted_thinking` block, and an Interactions `thought` step, verbatim (README §6.1).
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
    pub(crate) fn text_only(text: String) -> Self {
        Self {
            text,
            signature: None,
            opaque: Value::Null,
        }
    }
}

/// Reason a completion finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IrRole {
    /// The human side of the conversation.
    User,

    /// The model side of the conversation.
    Assistant,

    /// A tool result delivered back to the model.
    Tool,
}

/// Image content accepted from clients and rendered to upstreams.
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// Renders the image back into the OpenAI `image_url` form shared by the Chat Completions and
    /// Responses codecs; the inverse of [`IrImage::from_url`].
    pub(crate) fn to_url(&self) -> String {
        match self {
            Self::Inline {
                mime_type,
                data_base64,
            } => format!("data:{mime_type};base64,{data_base64}"),

            Self::Url(url) => url.clone(),
        }
    }
}

/// One typed segment of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
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
        /// only, dropped by every other surface (README §6.1).
        is_error: bool,
    },

    /// An assistant reasoning block (README §6.1): its text plus the opaque family-bound replay
    /// payload. Same-family round-trip replays the opaque payload verbatim; OpenAI surfaces
    /// render the text through the existing reasoning channels and drop the payload.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IrMessage {
    /// Participating role.
    pub(crate) role: IrRole,

    /// Ordered typed segments.
    pub(crate) parts: Vec<IrPart>,
}

/// A function tool advertised to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IrTool {
    /// Function name the model calls.
    pub(crate) name: String,

    /// Human-readable description of the function.
    pub(crate) description: Option<String>,

    /// JSON Schema of the accepted arguments.
    pub(crate) parameters: Option<Value>,
}

/// Restriction on which tool the model may call.
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// A fully format-independent completion request.
#[derive(Debug, Clone, Default)]
pub(crate) struct IrRequest {
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

    /// Reasoning effort level in the closed shared vocabulary (README §6.4): parsed from the
    /// client's own parameter into [`IrReasoningLevel`], written verbatim into the upstream's own
    /// parameter. `None` means the client omitted the field, and then no upstream parameter is
    /// rendered.
    pub(crate) reasoning_effort: Option<IrReasoningLevel>,
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
    /// signature so streaming and aggregated renderings agree (README §6.2). Upstream parsers
    /// emit either deltas plus an empty-text done, or a done carrying the full text with no
    /// prior deltas — never both with overlapping text — so renderers can render both safely:
    /// a non-empty `text` renders when no deltas preceded it, an empty `text` closes the block
    /// the deltas already delivered.
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
        /// surface renders it as `promptFeedback.blockReason` (README §6.3), every other
        /// surface ignores it.
        prompt_block_reason: Option<String>,
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
    /// payloads so streaming and aggregated renderings agree (README §6.2).
    pub(crate) reasoning_blocks: Vec<IrReasoningBlock>,

    /// Completed tool calls in emission order.
    pub(crate) tool_calls: Vec<IrAggregatedToolCall>,

    /// Reason the completion finished, when reported.
    pub(crate) finish: Option<IrFinishReason>,

    /// Verbatim upstream prompt-block reason when the finish is a prompt-level block
    /// (README §6.3); [`None`] for every other finish.
    pub(crate) prompt_block_reason: Option<String>,

    /// Token accounting, when reported.
    pub(crate) usage: Option<IrUsage>,
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
    use super::IrReasoningLevel;

    /// Pins both matches to [`IrReasoningLevel::VOCABULARY`]: every entry's wire spelling is
    /// exactly what [`as_str`](IrReasoningLevel::as_str) renders, and `parse(x.as_str())` returns
    /// `x` again.
    ///
    /// # Panics
    ///
    /// Panics if a table entry diverges from either match.
    #[test]
    fn vocabulary_round_trips_through_as_str_and_parse() {
        for (level, spelling) in IrReasoningLevel::VOCABULARY {
            assert_eq!(level.as_str(), spelling);

            #[expect(
                clippy::expect_used,
                reason = "a vocabulary spelling is constructed to parse"
            )]
            let parsed = IrReasoningLevel::parse(level.as_str(), "reasoning_effort")
                .expect("a vocabulary spelling must parse");

            assert_eq!(parsed, level);
        }
    }
}
