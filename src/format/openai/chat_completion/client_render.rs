//! Client-response rendering for the Chat Completions codec: renders IR events as Chat Completions
//! SSE frames or an aggregated non-streaming body.

use crate::{
    api::error::openai::OpenAiErrorEnvelope,
    format::{self, ClientSseRenderer},
    ids,
    ir::{IrAggregation, IrEvent, IrUsage},
    sse, unix_time,
};
use serde::Serialize;

/// The aggregated, non-streaming Chat Completions response body.
#[derive(Debug, Serialize)]
pub(crate) struct ChatCompletionResponse {
    /// Gateway-issued completion identifier.
    id: String,

    /// Object kind.
    object: &'static str,

    /// Unix creation timestamp.
    created: i64,

    /// Public model name the client addressed.
    model: String,

    /// The single served choice, read by the aggregated-body projection.
    pub(super) choices: Vec<ResponseChoice>,

    /// Token accounting.
    usage: TokenUsage,
}

/// Streaming client response renderer producing Chat Completions SSE frames.
pub(super) struct SseRenderer {
    /// Gateway-issued completion identifier.
    id: String,

    /// Public model name echoed to the client.
    model: String,

    /// Unix creation timestamp.
    created: i64,

    /// Whether the leading role chunk was emitted.
    is_role_sent: bool,

    /// Arguments already streamed, per opened tool-call index. An index's presence marks its call
    /// as opened by a delta chunk; a done for an index outside this map carries the complete call
    /// of an upstream that never streamed deltas, so it renders as one full tool_calls delta
    /// chunk. The accumulated prefixes let a conformant done render exactly the argument suffix
    /// the live deltas did not deliver — a done carries the complete arguments while an opening
    /// delta may carry none of them — and let the renderer recognize a done whose arguments the
    /// streamed bytes are not a prefix of (README §6.2).
    streamed_arguments: Vec<(usize, String)>,

    /// Outward ordinals of the tool calls seen so far, mapping each call's IR index to the dense
    /// outward `tool_calls[].index` its chunks render. The IR index is only the call's stable
    /// internal identity — a Responses upstream reports the item's `output_index`, its position
    /// among all output items, so a reasoning-plus-tool turn opens its first call at a nonzero IR
    /// index — while the outward Chat Completions contract requires a dense zero-based numbering
    /// per choice, so first sight of an IR index allocates the next free ordinal (README §6.2).
    outward_indexes: Vec<(usize, usize)>,

    /// Whether the trailing usage chunk is wanted.
    should_include_usage: bool,

    /// Latest token accounting observed; rendered once as the terminal usage chunk, because the
    /// official stream shape carries usage only on the final chunk before `[DONE]`.
    usage: Option<IrUsage>,
    /// Optional digest-only text projection and per-call outward assembly, never cache content.
    projection: Option<super::affinity::Output>,
}

/// The single aggregated choice.
#[derive(Debug, Serialize)]
pub(super) struct ResponseChoice {
    /// Always 0: the gateway serves one choice per request.
    index: u32,

    /// The complete assistant message.
    pub(super) message: AssistantMessage,

    /// Official `logprobs` member: required-nullable on the official choice schema, which the
    /// gateway never populates because it tracks no token probabilities, so the member renders
    /// `null` and stays present (README §6.3).
    logprobs: Option<ChoiceLogprobs>,

    /// Canonical finish reason; `stop` when the upstream reported none.
    finish_reason: &'static str,
}

/// The official choice `logprobs` object. Never rendered: the gateway tracks no token
/// probabilities, so [`ResponseChoice::logprobs`] is always `None`; the token-logprob element
/// shape is the official OpenAI shape, which the gateway never renders.
#[derive(Debug, Serialize)]
struct ChoiceLogprobs {
    /// Log probabilities of the message content tokens.
    content: [(); 0],

    /// Log probabilities of the message refusal tokens.
    refusal: [(); 0],
}

/// The complete aggregated assistant message.
#[derive(Debug, Serialize)]
pub(super) struct AssistantMessage {
    /// Always `assistant`.
    role: &'static str,

    /// The full assistant text, `null` for a turn that produced none.
    pub(super) content: Option<String>,

    /// Official `refusal` member: required-nullable on the official response message schema,
    /// which the gateway never populates because it never produces refusals, so the member
    /// renders `null` and stays present (README §6.3).
    refusal: Option<String>,

    /// The full reasoning text, omitted for a turn that produced none.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,

    /// Completed tool calls, omitted when the turn made none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) tool_calls: Vec<MessageToolCall>,
}

/// One completed tool call of an aggregated message.
#[derive(Debug, Serialize)]
pub(super) struct MessageToolCall {
    /// Invocation identifier.
    pub(super) id: String,

    /// Tool kind, `function`.
    #[serde(rename = "type")]
    tool_type: &'static str,

    /// The complete function invocation.
    pub(super) function: MessageFunction,

    /// Upstream thought signature the client must replay on the next turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) thought_signature: Option<String>,
}

/// The complete function invocation of an aggregated tool call.
#[derive(Debug, Serialize)]
pub(super) struct MessageFunction {
    /// Invoked function name.
    pub(super) name: String,

    /// Complete JSON-encoded arguments text.
    pub(super) arguments: String,
}

/// One streamed Chat Completions chunk (the `chat.completion.chunk` shape).
#[derive(Debug, Serialize)]
struct ChatCompletionChunk<'a> {
    /// Gateway-issued completion identifier.
    id: &'a str,

    /// Object kind.
    object: &'static str,

    /// Unix creation timestamp.
    created: i64,

    /// Public model name echoed to the client.
    model: &'a str,

    /// Per-choice deltas; empty on the terminal usage-only chunk, matching the official stream
    /// shape.
    choices: Vec<ChunkChoice>,

    /// Token accounting, carried only by the terminal usage-only chunk.
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<TokenUsage>,
}

/// One streamed choice: the incremental delta beside the finish reason, which only the terminal
/// content chunk carries.
#[derive(Debug, Serialize)]
struct ChunkChoice {
    /// Always 0: the gateway serves one choice per request.
    index: u32,

    /// The incremental delta.
    delta: Delta,

    /// Canonical finish reason, `null` on every non-terminal chunk.
    finish_reason: Option<&'static str>,
}

/// The incremental delta of one chunk. Absent fields are omitted rather than rendered as `null`,
/// exactly as the official stream shape carries them; the delta of the finishing chunk is empty.
#[derive(Debug, Default, Serialize)]
struct Delta {
    /// Role announcement, carried by the leading chunk only.
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'static str>,

    /// Assistant text; the leading chunk carries the empty string.
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,

    /// Reasoning text. Chat Completions has no official reasoning delta field; `reasoning_content`
    /// is the de-facto key that reasoner-aware OpenAI-compatible clients read (see DeepSeek's
    /// reasoning model guide).
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,

    /// Tool calls opened or extended by this delta.
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<DeltaToolCall>>,
}

/// One tool call opened or extended by a delta. The identity fields are carried by the chunk that
/// first reveals the call; an established call carries only its index beside whatever this chunk
/// adds.
#[derive(Debug, Serialize)]
struct DeltaToolCall {
    /// Stable index of the call within the response, renumbered outward-only into the dense
    /// zero-based ordinal of the choice's tool calls array (README §6.2).
    index: usize,

    /// Invocation identifier, revealed by the opening chunk.
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,

    /// Tool kind, `function`; revealed alongside the identifier.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    tool_type: Option<&'static str>,

    /// The function name and argument fragment this chunk delivers.
    #[serde(skip_serializing_if = "Option::is_none")]
    function: Option<DeltaFunction>,

    /// Upstream thought signature the client must replay on the next turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    thought_signature: Option<String>,
}

/// The function fragment of a tool-call delta: the name when this chunk first reveals it, beside
/// the argument bytes this chunk delivers.
#[derive(Debug, Serialize)]
struct DeltaFunction {
    /// Invoked function name, revealed by the opening chunk.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,

    /// Incremental JSON-encoded arguments text.
    arguments: String,
}

/// Token accounting in the flat Chat Completions shape, shared by the streamed terminal usage chunk
/// and the aggregated response body so streamed and non-streamed clients observe byte-identical
/// accounting (README §6.3).
#[derive(Debug, Serialize)]
struct TokenUsage {
    /// Tokens consumed by the input.
    prompt_tokens: u64,

    /// Tokens produced by the output.
    completion_tokens: u64,

    /// Sum of the two.
    total_tokens: u64,
}

impl TokenUsage {
    /// Renders one observation. The total adds with `saturating_add`: for every token count whose
    /// sum fits `u64` — every count a real upstream reports — the rendered total is the exact sum,
    /// and a third-party total can never panic (debug) or wrap (release) the renderer the way a
    /// plain add could at overflow.
    fn render(usage: IrUsage) -> Self {
        Self {
            prompt_tokens: usage.input_tokens,
            completion_tokens: usage.output_tokens,
            total_tokens: usage.input_tokens.saturating_add(usage.output_tokens),
        }
    }
}

impl SseRenderer {
    /// Creates a renderer for one streamed completion.
    pub(super) fn new(model: String, should_include_usage: bool) -> Self {
        Self {
            id: format!("chatcmpl-{}", ids::new_uuid()),
            model,
            created: unix_time::unix_now(),
            is_role_sent: false,
            streamed_arguments: Vec::new(),
            outward_indexes: Vec::new(),
            should_include_usage,
            usage: None,
            projection: None,
        }
    }

    /// Formats one delta chunk with an optional finish reason.
    fn chunk_frame(&self, delta: Delta, finish_reason: Option<&'static str>) -> String {
        let chunk = ChatCompletionChunk {
            id: &self.id,
            object: "chat.completion.chunk",
            created: self.created,
            model: &self.model,
            choices: vec![ChunkChoice {
                index: 0,
                delta,
                finish_reason,
            }],
            usage: None,
        };

        sse::format_data_frame(&format::render_json(&chunk))
    }

    /// Formats the terminal usage-only chunk carrying an empty choices array, matching the official
    /// stream shape.
    fn usage_frame(&self, usage: IrUsage) -> String {
        let chunk = ChatCompletionChunk {
            id: &self.id,
            object: "chat.completion.chunk",
            created: self.created,
            model: &self.model,
            choices: Vec::new(),
            usage: Some(TokenUsage::render(usage)),
        };

        sse::format_data_frame(&format::render_json(&chunk))
    }

    /// Returns the outward ordinal for the tool call the IR index identifies, allocating the next
    /// dense ordinal on first sight; every chunk of a call — deltas, residuals, and the done's
    /// full-call shape alike — renders the same ordinal (README §6.2).
    fn outward_index(&mut self, ir_index: usize) -> usize {
        if let Some((_, outward_index)) = self
            .outward_indexes
            .iter()
            .find(|(seen_index, _)| *seen_index == ir_index)
        {
            return *outward_index;
        }

        let outward_index = self.outward_indexes.len();
        self.outward_indexes.push((ir_index, outward_index));
        outward_index
    }

    /// Renders one tool-call delta chunk; a pooled projection observes the same wire fragments at
    /// the typed render point, without re-parsing the rendered frame.
    fn tool_delta_frame(&mut self, tool_call: DeltaToolCall) -> String {
        if let Some(projection) = self.projection.as_mut() {
            projection.call_delta(
                tool_call.index,
                tool_call.id.as_deref(),
                tool_call
                    .function
                    .as_ref()
                    .and_then(|part| part.name.as_deref()),
                tool_call
                    .function
                    .as_ref()
                    .map_or("", |part| part.arguments.as_str()),
                tool_call.thought_signature.as_deref(),
            );
        }

        self.chunk_frame(
            Delta {
                tool_calls: Some(vec![tool_call]),
                ..Delta::default()
            },
            None,
        )
    }
}

impl ClientSseRenderer for SseRenderer {
    /// Renders one IR event into zero or more SSE frames.
    fn render(&mut self, event: &IrEvent) -> Vec<String> {
        let frames = match event {
            // Native compaction state is unrepresentable on this surface and
            // unreachable in practice: a caller that cannot express compaction
            // controls can never trigger a native compaction upstream.
            IrEvent::CompactionState { .. } => Vec::new(),

            IrEvent::MessageStart { .. } => {
                if self.is_role_sent {
                    Vec::new()
                } else {
                    self.is_role_sent = true;

                    // The role chunk carries the empty string only; the projector ignores it.
                    vec![self.chunk_frame(
                        Delta {
                            role: Some("assistant"),
                            content: Some(String::new()),
                            ..Delta::default()
                        },
                        None,
                    )]
                }
            }

            IrEvent::TextDelta(delta) => {
                if let Some(projection) = self.projection.as_mut() {
                    projection.text(delta);
                }

                vec![self.chunk_frame(
                    Delta {
                        content: Some(delta.clone()),
                        ..Delta::default()
                    },
                    None,
                )]
            }

            IrEvent::ReasoningDelta(delta) => vec![self.chunk_frame(
                Delta {
                    reasoning_content: Some(delta.clone()),
                    ..Delta::default()
                },
                None,
            )],

            // A done carrying text renders it only when no deltas preceded it: upstream parsers
            // emit either deltas plus an empty-text done, or a done with the full text and no
            // deltas, never both with overlapping text — so rendering a non-empty done is the
            // done-only path, and the opaque payload is dropped on OpenAI surfaces (README §6.2).
            IrEvent::ReasoningDone { text, .. } => {
                if text.is_empty() {
                    Vec::new()
                } else {
                    vec![self.chunk_frame(
                        Delta {
                            reasoning_content: Some(text.clone()),
                            ..Delta::default()
                        },
                        None,
                    )]
                }
            }

            IrEvent::ToolCallDelta {
                index,
                call_id,
                name,
                arguments_delta,
            } => {
                match self
                    .streamed_arguments
                    .iter_mut()
                    .find(|(opened_index, _)| opened_index == index)
                {
                    Some((_, streamed)) => streamed.push_str(arguments_delta),

                    None => self
                        .streamed_arguments
                        .push((*index, arguments_delta.clone())),
                }

                let outward_index = self.outward_index(*index);

                let tool_call = DeltaToolCall {
                    index: outward_index,
                    id: call_id.clone(),
                    tool_type: call_id.as_ref().map(|_| "function"),
                    function: Some(DeltaFunction {
                        name: name.clone(),
                        arguments: arguments_delta.clone(),
                    }),
                    thought_signature: None,
                };

                vec![self.tool_delta_frame(tool_call)]
            }

            IrEvent::ToolCallDone {
                index,
                call_id,
                name,
                arguments,
                thought_signature,
            } => {
                let outward_index = self.outward_index(*index);

                let streamed = self
                    .streamed_arguments
                    .iter()
                    .find(|(opened_index, _)| opened_index == index)
                    .map(|(_, streamed)| streamed);

                match streamed {
                    None => {
                        // The upstream never streamed a delta for this index
                        // (README §6.2 recovery class), so the done payload
                        // carries the complete call and renders as one full
                        // tool_calls delta chunk, exactly the shape a delta
                        // would have produced.
                        let tool_call = DeltaToolCall {
                            index: outward_index,
                            id: Some(call_id.clone()),
                            tool_type: Some("function"),
                            function: Some(DeltaFunction {
                                name: Some(name.clone()),
                                arguments: arguments.clone(),
                            }),
                            thought_signature: thought_signature.clone(),
                        };

                        vec![self.tool_delta_frame(tool_call)]
                    }

                    Some(streamed) => {
                        let mut frames = Vec::new();

                        // The live deltas may have streamed only a prefix of the done's complete
                        // arguments — an opening delta alone carries none — so the done renders
                        // exactly the un-streamed suffix; a done whose arguments the deltas already
                        // streamed in full renders no argument chunk. Third-party bytes can diverge
                        // from that shape, and a residual cut from a non-prefix offset would splice
                        // onto the streamed prefix into arguments matching neither the deltas nor
                        // the done, so `strip_prefix` silently skips the residual argument frame of
                        // a done whose arguments are not a superstring of the streamed bytes
                        // (README §6.2); the skipped done's other content still renders.
                        //
                        // The residual is owned before rendering so the borrowed streamed prefix
                        // releases its borrow of the renderer before the mutable render point.
                        let residual = arguments
                            .strip_prefix(streamed.as_str())
                            .filter(|unstreamed| !unstreamed.is_empty())
                            .map(str::to_string);

                        if let Some(unstreamed) = residual {
                            frames.push(self.tool_delta_frame(DeltaToolCall {
                                index: outward_index,
                                id: None,
                                tool_type: None,
                                function: Some(DeltaFunction {
                                    name: None,
                                    arguments: unstreamed,
                                }),
                                thought_signature: None,
                            }));
                        }

                        if let Some(signature) = thought_signature {
                            frames.push(self.tool_delta_frame(DeltaToolCall {
                                index: outward_index,
                                id: None,
                                tool_type: None,
                                function: None,
                                thought_signature: Some(signature.clone()),
                            }));
                        }

                        frames
                    }
                }
            }

            IrEvent::Finish { reason, .. } => {
                vec![self.chunk_frame(
                    Delta::default(),
                    Some(super::finish_reason_to_openai(*reason)),
                )]
            }

            IrEvent::CompactionMetadata { .. } => Vec::new(),
            IrEvent::Usage(usage) => {
                // Gemini upstreams attach cumulative usage to every stream chunk; the latest
                // observation wins and renders once, terminally.
                self.usage = Some(*usage);

                Vec::new()
            }

            IrEvent::Done => {
                let mut frames = Vec::new();

                if self.should_include_usage
                    && let Some(usage) = self.usage
                {
                    frames.push(self.usage_frame(usage));
                }

                frames.push(sse::format_data_frame("[DONE]"));

                frames
            }
        };
        // A done completes its call after the frames carrying its final fragments rendered.
        if let IrEvent::ToolCallDone { index, .. } = event
            && let Some((_, outward_index)) = self
                .outward_indexes
                .iter()
                .find(|(ir_index, _)| ir_index == index)
            && let Some(projection) = self.projection.as_mut()
        {
            projection.complete_call(*outward_index);
        }
        frames
    }

    fn begin_projection(&mut self, projector: crate::affinity::fingerprint::Projector) {
        self.projection = Some(super::affinity::Output::new(projector));
    }

    fn take_projection(&mut self) -> Option<crate::affinity::fingerprint::Projector> {
        self.projection
            .take()
            .and_then(super::affinity::Output::finish)
    }

    /// Renders a mid-stream error as a terminal SSE frame carrying the OpenAI error envelope,
    /// classified once through the shared API error (README §6.7).
    fn error_frame(&mut self, error: &crate::api::Error) -> String {
        let envelope = OpenAiErrorEnvelope {
            error: crate::api::error::openai::render_body(error),
        };

        sse::format_data_frame(&format::render_json(&envelope))
    }
}

/// Renders an aggregated completion as a non-streaming Chat Completions response.
pub(super) fn render_json_response(
    model: &str,
    aggregation: &IrAggregation,
) -> ChatCompletionResponse {
    let tool_calls = aggregation
        .tool_calls
        .iter()
        .map(|tool_call| MessageToolCall {
            id: tool_call.call_id.clone(),
            tool_type: "function",
            function: MessageFunction {
                name: tool_call.name.clone(),
                arguments: tool_call.arguments.clone(),
            },
            thought_signature: tool_call.thought_signature.clone(),
        })
        .collect();

    let message = AssistantMessage {
        role: "assistant",
        content: (!aggregation.text.is_empty()).then(|| aggregation.text.clone()),
        refusal: None,
        reasoning_content: (!aggregation.reasoning.is_empty())
            .then(|| aggregation.reasoning.clone()),
        tool_calls,
    };

    ChatCompletionResponse {
        id: format!("chatcmpl-{}", ids::new_uuid()),
        object: "chat.completion",
        created: unix_time::unix_now(),
        model: model.to_string(),
        choices: vec![ResponseChoice {
            index: 0,
            message,
            logprobs: None,
            finish_reason: aggregation
                .finish
                .map(super::finish_reason_to_openai)
                .unwrap_or("stop"),
        }],
        usage: TokenUsage::render(aggregation.usage.unwrap_or_default()),
    }
}
