//! Client-response rendering for the Chat Completions codec: renders IR events as Chat Completions
//! SSE frames or an aggregated non-streaming body.

use crate::{
    format::{self, ClientSseRenderer, openai::error::OpenAiErrorEnvelope},
    ids,
    ir::{IrAggregation, IrEvent, IrUsage},
    sse, unix_time,
};
use serde::Serialize;
use serde_json::Value;

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

    /// The single served choice.
    choices: Vec<ResponseChoice>,

    /// Token accounting.
    usage: TokenUsage,
}

/// Streaming client response renderer producing Chat Completions SSE frames.
#[derive(Debug)]
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
}

/// The single aggregated choice.
#[derive(Debug, Serialize)]
struct ResponseChoice {
    /// Always 0: the gateway serves one choice per request.
    index: u32,

    /// The complete assistant message.
    message: AssistantMessage,

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
    content: Vec<Value>,

    /// Log probabilities of the message refusal tokens.
    refusal: Vec<Value>,
}

/// The complete aggregated assistant message.
#[derive(Debug, Serialize)]
struct AssistantMessage {
    /// Always `assistant`.
    role: &'static str,

    /// The full assistant text, `null` for a turn that produced none.
    content: Option<String>,

    /// Official `refusal` member: required-nullable on the official response message schema,
    /// which the gateway never populates because it never produces refusals, so the member
    /// renders `null` and stays present (README §6.3).
    refusal: Option<String>,

    /// The full reasoning text, omitted for a turn that produced none.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,

    /// Completed tool calls, omitted when the turn made none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<MessageToolCall>,
}

/// One completed tool call of an aggregated message.
#[derive(Debug, Serialize)]
struct MessageToolCall {
    /// Invocation identifier.
    id: String,

    /// Tool kind, `function`.
    #[serde(rename = "type")]
    tool_type: &'static str,

    /// The complete function invocation.
    function: MessageFunction,

    /// Upstream thought signature the client must replay on the next turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    thought_signature: Option<String>,
}

/// The complete function invocation of an aggregated tool call.
#[derive(Debug, Serialize)]
struct MessageFunction {
    /// Invoked function name.
    name: String,

    /// Complete JSON-encoded arguments text.
    arguments: String,
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
    /// Renders one observation.
    fn render(usage: IrUsage) -> Self {
        Self {
            prompt_tokens: usage.input_tokens,
            completion_tokens: usage.output_tokens,
            total_tokens: usage.input_tokens + usage.output_tokens,
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
}

impl ClientSseRenderer for SseRenderer {
    /// Renders one IR event into zero or more SSE frames.
    fn render(&mut self, event: &IrEvent) -> Vec<String> {
        match event {
            // Native compaction state is unrepresentable on this surface and
            // unreachable in practice: a caller that cannot express compaction
            // controls can never trigger a native compaction upstream.
            IrEvent::CompactionState { .. } => Vec::new(),

            IrEvent::MessageStart { .. } => {
                if self.is_role_sent {
                    Vec::new()
                } else {
                    self.is_role_sent = true;

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

            IrEvent::TextDelta(delta) => vec![self.chunk_frame(
                Delta {
                    content: Some(delta.clone()),
                    ..Delta::default()
                },
                None,
            )],

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

                vec![self.chunk_frame(
                    Delta {
                        tool_calls: Some(vec![tool_call]),
                        ..Delta::default()
                    },
                    None,
                )]
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

                        vec![self.chunk_frame(
                            Delta {
                                tool_calls: Some(vec![tool_call]),
                                ..Delta::default()
                            },
                            None,
                        )]
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
                        if let Some(unstreamed) = arguments.strip_prefix(streamed.as_str())
                            && !unstreamed.is_empty()
                        {
                            frames.push(self.chunk_frame(
                                Delta {
                                    tool_calls: Some(vec![DeltaToolCall {
                                        index: outward_index,
                                        id: None,
                                        tool_type: None,
                                        function: Some(DeltaFunction {
                                            name: None,
                                            arguments: unstreamed.to_string(),
                                        }),
                                        thought_signature: None,
                                    }]),
                                    ..Delta::default()
                                },
                                None,
                            ));
                        }

                        if let Some(signature) = thought_signature {
                            frames.push(self.chunk_frame(
                                Delta {
                                    tool_calls: Some(vec![DeltaToolCall {
                                        index: outward_index,
                                        id: None,
                                        tool_type: None,
                                        function: None,
                                        thought_signature: Some(signature.clone()),
                                    }]),
                                    ..Delta::default()
                                },
                                None,
                            ));
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
        }
    }

    /// Renders a mid-stream error as a terminal SSE frame carrying the OpenAI error envelope,
    /// classified once through the shared API error (README §6.7).
    fn error_frame(&mut self, error: &crate::api::Error) -> String {
        let envelope = OpenAiErrorEnvelope {
            error: crate::format::openai::error::render_body(error),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{format::aggregated_body_json, ir::IrFinishReason};
    use serde_json::Value;

    /// Parses the JSON payload of every rendered SSE frame.
    ///
    /// # Panics
    ///
    /// Panics if a rendered frame is missing or its payload is not JSON.
    #[expect(
        clippy::expect_used,
        reason = "the rendered frames are under test; a missing or malformed frame is the failure"
    )]
    fn frame_payloads(frames: &[String]) -> Vec<Value> {
        frames
            .iter()
            .map(|frame| {
                serde_json::from_str(
                    frame
                        .strip_prefix("data: ")
                        .expect("every frame carries a data line")
                        .trim_end(),
                )
                .expect("frame data is rendered JSON")
            })
            .collect()
    }

    /// Joins the tool-call argument fragments a rendered frame set delivers,
    /// exactly as a client assembling the stream would.
    ///
    /// # Panics
    ///
    /// Panics if a rendered frame lacks exactly one tool call with string
    /// arguments.
    #[expect(
        clippy::expect_used,
        reason = "the rendered frames are under test; a missing or malformed frame is the failure"
    )]
    fn assembled_arguments(frames: &[String]) -> String {
        frame_payloads(frames)
            .iter()
            .map(|payload| {
                payload
                    .pointer("/choices/0/delta/tool_calls/0/function/arguments")
                    .expect("every rendered chunk carries one tool call")
                    .as_str()
                    .expect("tool-call arguments render as a JSON string")
                    .to_string()
            })
            .collect()
    }

    /// Reads the outward tool-call index each rendered frame delivers,
    /// exactly as a client assembling the stream would.
    ///
    /// # Panics
    ///
    /// Panics if a rendered frame lacks exactly one tool call carrying a
    /// numeric index.
    #[expect(
        clippy::expect_used,
        reason = "the rendered frames are under test; a missing or malformed frame is the failure"
    )]
    fn outward_tool_call_indexes(frames: &[String]) -> Vec<usize> {
        frame_payloads(frames)
            .iter()
            .map(|payload| {
                payload
                    .pointer("/choices/0/delta/tool_calls/0/index")
                    .expect("every rendered chunk carries one tool call")
                    .as_u64()
                    .expect("a tool-call index renders as a JSON number") as usize
            })
            .collect()
    }

    /// Parses the JSON payload of the only rendered SSE frame.
    ///
    /// # Panics
    ///
    /// Panics if the event renders no frame or more than one.
    #[expect(
        clippy::expect_used,
        reason = "the rendered frames are under test; a missing or malformed frame is the failure"
    )]
    fn lone_frame_payload(frames: &[String]) -> Value {
        let mut payloads = frame_payloads(frames);

        let payload = payloads.pop().expect("the event renders a frame");

        assert!(payloads.is_empty(), "the event renders exactly one frame");

        payload
    }

    /// A call opened by an empty delta — the Responses upstream opens every
    /// function call with a zero-byte `output_item.added` delta — receives
    /// its full arguments from the done event exactly once, because a
    /// streamed done always carries the authoritative complete arguments
    /// (README §6.2).
    ///
    /// # Panics
    ///
    /// Panics if the rendered stream diverges from the documented behavior.
    #[test]
    fn done_renders_arguments_the_opening_delta_never_streamed() {
        let mut renderer = SseRenderer::new("m".to_string(), false);

        let opened = renderer.render(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("call_1".to_string()),
            name: Some("get_weather".to_string()),
            arguments_delta: String::new(),
        });

        let done = renderer.render(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: r#"{"city":"Paris","unit":"c"}"#.to_string(),
            thought_signature: None,
        });

        assert_eq!(assembled_arguments(&opened), "");

        assert_eq!(outward_tool_call_indexes(&opened), vec![0]);

        assert_eq!(assembled_arguments(&done), r#"{"city":"Paris","unit":"c"}"#);

        assert_eq!(outward_tool_call_indexes(&done), vec![0]);
    }

    /// Live deltas that already streamed the full arguments — the Gemini
    /// upstream delivers every call this way — are not repeated by the done
    /// event, so the client assembles the arguments exactly once.
    ///
    /// # Panics
    ///
    /// Panics if the rendered stream diverges from the documented behavior.
    #[test]
    fn done_repeats_no_arguments_the_live_deltas_already_streamed() {
        let mut renderer = SseRenderer::new("m".to_string(), false);

        let opened = renderer.render(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("call_1".to_string()),
            name: Some("get_weather".to_string()),
            arguments_delta: r#"{"city":"Paris"}"#.to_string(),
        });

        let done = renderer.render(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: r#"{"city":"Paris"}"#.to_string(),
            thought_signature: None,
        });

        assert_eq!(assembled_arguments(&opened), r#"{"city":"Paris"}"#);

        assert_eq!(assembled_arguments(&done), "");

        assert!(done.is_empty());
    }

    /// A divergent upstream whose done arguments are not a superstring of
    /// the streamed bytes — shorter than the streamed bytes, or diverging
    /// from them at a byte the deltas did not stream — loses its residual
    /// arguments silently: the done skips its argument frame (README
    /// §6.2), while its other content still renders.
    ///
    /// # Panics
    ///
    /// Panics if the rendered stream diverges from the documented behavior.
    #[test]
    fn done_with_arguments_divergent_from_the_streamed_bytes_skips_the_argument_frame() {
        let mut renderer = SseRenderer::new("m".to_string(), false);

        let opened = renderer.render(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("call_1".to_string()),
            name: Some("get_weather".to_string()),
            arguments_delta: r#"{"city":"Paris"}"#.to_string(),
        });

        // The done is shorter than the streamed bytes, so the streamed
        // bytes are not a prefix of its arguments.
        let shorter = renderer.render(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        });

        let reopened = renderer.render(&IrEvent::ToolCallDelta {
            index: 1,
            call_id: Some("call_2".to_string()),
            name: Some("get_weather".to_string()),
            arguments_delta: "é".to_string(),
        });

        // The streamed bytes "é" are not a byte-prefix of the done's
        // "xé" — it diverges at the first byte — so the residual argument
        // frame is skipped; the done's thought signature still renders.
        let divergent = renderer.render(&IrEvent::ToolCallDone {
            index: 1,
            call_id: "call_2".to_string(),
            name: "get_weather".to_string(),
            arguments: "xé".to_string(),
            thought_signature: Some("sig".to_string()),
        });

        assert_eq!(assembled_arguments(&opened), r#"{"city":"Paris"}"#);

        assert!(shorter.is_empty());

        assert_eq!(assembled_arguments(&reopened), "é");

        let payload = lone_frame_payload(&divergent);

        assert_eq!(
            payload.pointer("/choices/0/delta/tool_calls/0/thought_signature"),
            Some(&serde_json::json!("sig"))
        );

        assert!(
            payload
                .pointer("/choices/0/delta/tool_calls/0/function")
                .is_none()
        );
    }

    /// A done that is longer than, but divergent from, the streamed bytes
    /// renders no residual: a suffix cut past a non-prefix offset would
    /// splice onto the streamed prefix into arguments matching neither the
    /// deltas nor the done, so the divergent done silently skips its
    /// argument frame (README §6.2).
    ///
    /// # Panics
    ///
    /// Panics if the rendered stream diverges from the documented behavior.
    #[test]
    fn done_longer_but_divergent_from_the_streamed_bytes_skips_the_argument_frame() {
        let mut renderer = SseRenderer::new("m".to_string(), false);

        let opened = renderer.render(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("call_1".to_string()),
            name: Some("get_weather".to_string()),
            arguments_delta: "abc".to_string(),
        });

        let done = renderer.render(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: "xbcd".to_string(),
            thought_signature: None,
        });

        assert_eq!(assembled_arguments(&opened), "abc");

        assert!(done.is_empty());
    }

    /// A done for an IR index no delta ever opened — the recovery class of
    /// README §6.2 — renders the complete call as one full tool_calls delta
    /// chunk, exactly the shape a delta would have produced, under the
    /// outward ordinal its first sight allocates: index 0 for the first-seen
    /// IR index.
    ///
    /// # Panics
    ///
    /// Panics if the rendered stream diverges from the documented behavior.
    #[test]
    fn done_without_a_prior_delta_renders_the_complete_call() {
        let mut renderer = SseRenderer::new("m".to_string(), false);

        let done = renderer.render(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_3".to_string(),
            name: "get_weather".to_string(),
            arguments: "{}".to_string(),
            thought_signature: Some("sig".to_string()),
        });

        assert_eq!(
            lone_frame_payload(&done).get("choices"),
            Some(&serde_json::json!([{
                "index": 0,
                "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "call_3",
                    "type": "function",
                    "function": {"name": "get_weather", "arguments": "{}"},
                    "thought_signature": "sig",
                }]},
                "finish_reason": null,
            }]))
        );
    }

    /// Tool-call chunks renumber the calls outward-only: IR indexes that
    /// start at 1 and skip — a Responses upstream reports each item's
    /// `output_index`, its position among all output items — render as dense
    /// outward indexes from zero, and a done for an opened call reuses the
    /// outward ordinal its deltas opened.
    ///
    /// # Panics
    ///
    /// Panics if the rendered stream diverges from the documented behavior.
    #[test]
    fn sparse_ir_indexes_render_dense_outward_indexes() {
        let mut renderer = SseRenderer::new("m".to_string(), false);

        let first = renderer.render(&IrEvent::ToolCallDelta {
            index: 1,
            call_id: Some("call_1".to_string()),
            name: Some("get_weather".to_string()),
            arguments_delta: String::new(),
        });

        let second = renderer.render(&IrEvent::ToolCallDelta {
            index: 3,
            call_id: Some("call_2".to_string()),
            name: Some("get_weather".to_string()),
            arguments_delta: r#"{"city":"Paris"}"#.to_string(),
        });

        // The done carries the full arguments the empty opening delta never
        // streamed, so it renders the residual frame at the call's outward
        // ordinal.
        let first_done = renderer.render(&IrEvent::ToolCallDone {
            index: 1,
            call_id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: r#"{"city":"Paris","unit":"c"}"#.to_string(),
            thought_signature: None,
        });

        // The second call's deltas already streamed its full arguments, so
        // its done renders nothing.
        let second_done = renderer.render(&IrEvent::ToolCallDone {
            index: 3,
            call_id: "call_2".to_string(),
            name: "get_weather".to_string(),
            arguments: r#"{"city":"Paris"}"#.to_string(),
            thought_signature: None,
        });

        assert_eq!(outward_tool_call_indexes(&first), vec![0]);

        assert_eq!(outward_tool_call_indexes(&second), vec![1]);

        assert_eq!(outward_tool_call_indexes(&first_done), vec![0]);

        assert_eq!(
            assembled_arguments(&first_done),
            r#"{"city":"Paris","unit":"c"}"#
        );

        assert!(second_done.is_empty());
    }

    /// A done for an IR index no delta ever opened allocates the next dense
    /// outward ordinal — after the ordinal the opened IR index took — and
    /// renders the complete call under it.
    ///
    /// # Panics
    ///
    /// Panics if the rendered stream diverges from the documented behavior.
    #[test]
    fn done_for_a_never_opened_index_allocates_the_next_outward_ordinal() {
        let mut renderer = SseRenderer::new("m".to_string(), false);

        let opened = renderer.render(&IrEvent::ToolCallDelta {
            index: 1,
            call_id: Some("call_1".to_string()),
            name: Some("get_weather".to_string()),
            arguments_delta: r#"{"city":"Paris"}"#.to_string(),
        });

        let done = renderer.render(&IrEvent::ToolCallDone {
            index: 4,
            call_id: "call_5".to_string(),
            name: "get_time".to_string(),
            arguments: "{}".to_string(),
            thought_signature: Some("sig".to_string()),
        });

        assert_eq!(outward_tool_call_indexes(&opened), vec![0]);

        let payload = lone_frame_payload(&done);

        assert_eq!(
            payload.pointer("/choices/0/delta/tool_calls/0/index"),
            Some(&serde_json::json!(1))
        );

        assert_eq!(
            payload.pointer("/choices/0/delta/tool_calls/0/id"),
            Some(&serde_json::json!("call_5"))
        );
    }

    /// The aggregated choice carries every required member of the official
    /// chat completion object shape, including the required-nullable members
    /// the gateway always renders empty — the choice's `logprobs` (object or
    /// null) and the message's `refusal` (string or null), which the official
    /// schema and openai-node both type as required and which the gateway
    /// never produces (README §6.3).
    ///
    /// # Panics
    ///
    /// Panics if the rendered body diverges from the official shape.
    #[test]
    fn aggregated_body_carries_the_required_nullable_choice_and_message_members() {
        let aggregation = IrAggregation {
            text: "answer".to_string(),
            finish: Some(IrFinishReason::Stop),
            ..IrAggregation::default()
        };

        let body = aggregated_body_json(&render_json_response("m", &aggregation));

        assert_eq!(
            body.pointer("/choices/0"),
            Some(&serde_json::json!({
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "answer",
                    "refusal": null,
                },
                "logprobs": null,
                "finish_reason": "stop",
            }))
        );
    }
}
