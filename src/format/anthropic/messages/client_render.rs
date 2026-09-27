//! Client-response rendering for the Anthropic Messages codec: renders IR events as native
//! Messages SSE frames (`message_start`, `content_block_*`, `message_delta`, `message_stop`) or an
//! aggregated non-streaming response.

use super::{OutBlock, TypedBlock, is_anthropic_block};
use crate::{
    format::{
        ClientSseRenderer,
        anthropic::error::{AnthropicErrorBody, render_body},
        arguments_to_object_or_empty,
    },
    ids,
    ir::{IrAggregation, IrEvent, IrFinishReason, IrUsage},
    sse,
};
use serde::Serialize;
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

    /// Response content: thinking blocks first, then text, then tool calls.
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
/// the output reasoning breakdown when the upstream reported it.
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
        }
    }
}

/// Renders an aggregated completion as a non-streaming Messages response.
pub(super) fn render_json_response(model: &str, aggregation: &IrAggregation) -> MessagesResponse {
    let mut content: Vec<OutBlock> = Vec::new();

    for block in &aggregation.reasoning_blocks {
        if is_anthropic_block(&block.opaque) {
            content.push(OutBlock::Verbatim(block.opaque.clone()));
        } else if !block.text.is_empty() {
            content.push(OutBlock::Typed(TypedBlock::Thinking {
                thinking: block.text.clone(),
                signature: block.signature.clone(),
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
        stop_reason: aggregation
            .finish
            .map(finish_reason_to_stop)
            .unwrap_or("end_turn"),
        stop_sequence: None,
        usage: aggregation
            .usage
            .map(ResponseUsage::render)
            .unwrap_or_else(|| ResponseUsage::render(IrUsage::default())),
    }
}
/// Renders the canonical reason as an Anthropic `stop_reason` string.
pub(super) fn finish_reason_to_stop(reason: IrFinishReason) -> &'static str {
    match reason {
        IrFinishReason::Stop => "end_turn",
        IrFinishReason::Length => "max_tokens",
        IrFinishReason::ToolCall => "tool_use",
        IrFinishReason::ContentFilter => "refusal",
    }
}

/// Streaming client response renderer producing Messages SSE frames.
#[derive(Debug)]
pub(super) struct SseRenderer {
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

    /// Tool calls buffered from deltas, emitted whole on their done event so the `tool_use`
    /// start event carries its authoritative identity.
    tool_buffers: Vec<ToolBuffer>,

    /// Latest token accounting observed; rendered in the terminal delta.
    usage: Option<IrUsage>,

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
    content: Vec<Value>,

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

    /// Thinking block opening empty; the signature streams later as a delta.
    Thinking {
        /// Always empty at the start.
        thinking: &'static str,
    },

    /// Tool invocation block with its identity; arguments stream as deltas.
    ToolUse {
        /// Invocation identifier.
        id: String,

        /// Invoked function name.
        name: String,

        /// Always empty at the start.
        input: Value,
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
    /// Canonical stop reason.
    stop_reason: &'static str,

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
    pub(super) fn new(model: String) -> Self {
        Self {
            id: format!("msg_{}", ids::new_uuid()),
            model,
            next_index: 0,
            open_block: None,
            is_started: false,
            is_start_buffered: false,
            tool_buffers: Vec::new(),
            usage: None,
            finish: None,
        }
    }

    /// Formats one SSE frame with its event name and typed payload.
    fn frame<T>(event: &'static str, payload: &T) -> String
    where
        T: Serialize + ?Sized,
    {
        sse::format_frame(Some(event), &crate::format::render_json(payload))
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
                content_block: StartBlockOut::Thinking { thinking: "" },
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
                    content: Vec::new(),
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
    /// deliver usage after the finish reason, so only Done-time usage is final.
    fn message_delta_frame(&self, reason: IrFinishReason) -> String {
        Self::frame(
            "message_delta",
            &MessageDeltaPayload {
                kind: "message_delta",
                delta: MessageDelta {
                    stop_reason: finish_reason_to_stop(reason),
                    stop_sequence: None,
                },
                usage: self
                    .usage
                    .map(ResponseUsage::render)
                    .unwrap_or_else(|| ResponseUsage::render(IrUsage::default())),
            },
        )
    }
}

impl ClientSseRenderer for SseRenderer {
    /// Renders one IR event into zero or more SSE frames.
    fn render(&mut self, event: &IrEvent) -> Vec<String> {
        // One-event lookahead: the head stays buffered until the next event resolves it, so
        // the stream-head partial usage report — published immediately after
        // [`IrEvent::MessageStart`] per the IR contract (README §6.2) — folds its input and
        // cache columns into the opening usage; every other event resolves the buffer with
        // zeros first, keeping `message_start` exactly once and first in every stream.
        let mut frames = Vec::new();

        if self.is_start_buffered {
            self.is_start_buffered = false;

            let usage = match event {
                IrEvent::Usage(reported) => ResponseUsage::from_head_report(*reported),
                _ => ResponseUsage::zero(),
            };

            frames.push(self.message_start_frame(usage));
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
                            input: Value::Object(Default::default()),
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
    fn error_frame(&mut self, error: &crate::api::Error) -> String {
        let head = if self.is_start_buffered {
            self.is_start_buffered = false;

            self.message_start_frame(ResponseUsage::zero())
        } else {
            String::new()
        };

        head + &Self::frame(
            "error",
            &StreamErrorPayload {
                kind: "error",
                error: render_body(error),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ir::IrReasoningBlock, test_support::at};
    use serde_json::{Value, json};

    /// Reads the first rendered frame payload.
    ///
    /// # Panics
    ///
    /// Panics if no payload was rendered: a missing frame is the failure under test.
    fn first_payload(payloads: &[Value]) -> &Value {
        #[expect(
            clippy::expect_used,
            reason = "rendered frames are under test; a missing frame is the failure"
        )]
        payloads
            .first()
            .expect("the event renders at least one frame")
    }

    /// Reads one rendered frame payload by position.
    ///
    /// # Panics
    ///
    /// Panics if the position is absent: a missing frame is the failure under test.
    fn payload_at(payloads: &[Value], index: usize) -> &Value {
        #[expect(
            clippy::expect_used,
            reason = "rendered frames are under test; a missing frame is the failure"
        )]
        payloads
            .get(index)
            .expect("the event renders the expected frame")
    }

    /// Lists the event names of rendered frames in stream order.
    fn kinds(frames: &[String]) -> Vec<String> {
        frames
            .iter()
            .map(|frame| {
                frame
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim_start_matches("event: ")
                    .to_string()
            })
            .collect()
    }

    /// Parses each rendered frame's JSON payload in stream order.
    ///
    /// # Panics
    ///
    /// Panics if a frame carries no data line or its data is not rendered JSON: a malformed
    /// frame is the failure under test.
    fn payloads(frames: &[String]) -> Vec<Value> {
        frames
            .iter()
            .map(|frame| {
                #[expect(
                    clippy::expect_used,
                    reason = "the rendered frames are under test; a malformed frame is the failure"
                )]
                serde_json::from_str::<Value>(
                    frame
                        .split_once("data: ")
                        .expect("every frame carries a data line")
                        .1,
                )
                .expect("frame data is rendered JSON")
            })
            .collect()
    }

    /// Renders one aggregated Messages response: thinking replays verbatim, text and tool calls
    /// follow in order, the stop reason and cache columns ride through, and reasoning text is
    /// never duplicated between blocks and prose.
    ///
    /// # Panics
    ///
    /// Panics if the rendered body diverges from the native shape.
    #[test]
    fn renders_the_aggregated_response_without_duplicating_reasoning() {
        let opaque: Value = json!({
            "type": "thinking",
            "thinking": "upstream",
            "signature": "up-sig",
        });

        let aggregation = IrAggregation {
            text: "answer".to_string(),
            reasoning: "upstream".to_string(),
            reasoning_blocks: vec![
                crate::ir::IrReasoningBlock {
                    text: "upstream".to_string(),
                    signature: Some("up-sig".to_string()),
                    opaque,
                },
                IrReasoningBlock::text_only("foreign thought".to_string()),
            ],
            tool_calls: vec![crate::ir::IrAggregatedToolCall {
                call_id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: r#"{"q":"x"}"#.to_string(),
                thought_signature: None,
            }],
            finish: Some(IrFinishReason::ToolCall),
            prompt_block_reason: None,
            usage: Some(IrUsage {
                input_tokens: 4,
                output_tokens: 8,
                cached_tokens: Some(1),
                cache_write_tokens: None,
                reasoning_tokens: Some(3),
            }),
        };

        #[expect(
            clippy::expect_used,
            reason = "the aggregated body is under test; a failing render is the failure"
        )]
        let body = serde_json::to_value(render_json_response("claude-test", &aggregation))
            .expect("the aggregated body serializes");

        assert_eq!(at(&body, "/id").as_str().map(str::len), Some(40));
        assert!(
            at(&body, "/id")
                .as_str()
                .is_some_and(|id| id.starts_with("msg_"))
        );
        assert_eq!(at(&body, "/type"), &json!("message"));
        assert_eq!(at(&body, "/role"), &json!("assistant"));
        assert_eq!(at(&body, "/model"), &json!("claude-test"));
        assert_eq!(at(&body, "/stop_reason"), &json!("tool_use"));
        assert_eq!(at(&body, "/stop_sequence"), &Value::Null);

        // The verbatim block replays exactly; the foreign block synthesizes thinking; the
        // reasoning text appears only inside thinking blocks, never as prose.
        assert_eq!(
            at(&body, "/content"),
            &json!([
                {"type": "thinking", "thinking": "upstream", "signature": "up-sig"},
                {"type": "thinking", "thinking": "foreign thought"},
                {"type": "text", "text": "answer"},
                {"type": "tool_use", "id": "call_1", "name": "search",
                 "input": {"q": "x"}},
            ]),
        );

        // Cache columns ride through when reported, and the upstream-reported reasoning
        // breakdown renders under the native `output_tokens_details.thinking_tokens` spelling.
        assert_eq!(
            at(&body, "/usage"),
            &json!({
                "input_tokens": 4,
                "output_tokens": 8,
                "cache_read_input_tokens": 1,
                "output_tokens_details": {"thinking_tokens": 3},
            }),
        );
    }

    /// Streams one turn through the client renderer: the `message_start` head waits one event
    /// and resolves on the first event after the start — zero usage when no partial report
    /// preceded — block indexes ascend across text, thinking, and tool blocks; the signature
    /// closes the thinking block; the tool call emits whole on its done event; and the
    /// terminal delta rides `Done` — after the usage that cross-format upstreams report
    /// following the finish — carrying the stop reason and the final usage before
    /// `message_stop`.
    ///
    /// # Panics
    ///
    /// Panics if the rendered frame sequence diverges from the native stream shape.
    #[test]
    fn streams_native_frames_with_ascending_block_indexes() {
        let mut renderer = SseRenderer::new("claude-test".to_string());

        // The head is buffered one event for the lookahead: the start alone renders nothing.
        let buffered = renderer.render(&IrEvent::MessageStart {
            id: "up".to_string(),
            model: "up-model".to_string(),
        });

        assert!(buffered.is_empty());

        let text = renderer.render(&IrEvent::TextDelta("hi".to_string()));

        assert_eq!(
            kinds(&text),
            [
                "message_start",
                "content_block_start",
                "content_block_delta",
            ],
        );

        let text_payloads = payloads(&text);

        // No partial report preceded the first delta: the head opens with zero usage.
        assert_eq!(
            at(first_payload(&text_payloads), "/message/usage"),
            &json!({"input_tokens": 0, "output_tokens": 0}),
        );
        assert_eq!(
            at(first_payload(&text_payloads), "/message/model"),
            &json!("claude-test")
        );
        assert_eq!(at(payload_at(&text_payloads, 1), "/index"), &json!(0));
        assert_eq!(
            at(payload_at(&text_payloads, 2), "/delta"),
            &json!({"type": "text_delta", "text": "hi"}),
        );

        let thinking = renderer.render(&IrEvent::ReasoningDelta("draft".to_string()));

        assert_eq!(
            kinds(&thinking),
            [
                "content_block_stop",
                "content_block_start",
                "content_block_delta",
            ],
        );

        let done = renderer.render(&IrEvent::ReasoningDone {
            text: String::new(),
            signature: Some("sig".to_string()),
            opaque: Value::Null,
        });

        assert_eq!(kinds(&done), ["content_block_delta", "content_block_stop"]);

        let done_payloads = payloads(&done);

        assert_eq!(
            at(payload_at(&done_payloads, 0), "/delta"),
            &json!({"type": "signature_delta", "signature": "sig"}),
        );

        // Tool deltas buffer silently; the done event emits the whole block.
        let buffered = renderer.render(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("call_1".to_string()),
            name: Some("search".to_string()),
            arguments_delta: "{\"q\":".to_string(),
        });

        assert!(buffered.is_empty());

        let tool_done = renderer.render(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "search".to_string(),
            arguments: "{\"q\":\"x\"}".to_string(),
            thought_signature: None,
        });

        assert_eq!(
            kinds(&tool_done),
            [
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
            ],
        );

        let tool_payloads = payloads(&tool_done);

        assert_eq!(at(payload_at(&tool_payloads, 0), "/index"), &json!(2));
        assert_eq!(
            at(payload_at(&tool_payloads, 0), "/content_block"),
            &json!({"type": "tool_use", "id": "call_1", "name": "search",
                   "input": {}}),
        );
        assert_eq!(
            at(payload_at(&tool_payloads, 1), "/delta"),
            &json!({"type": "input_json_delta", "partial_json": "{\"q\":\"x\"}"}),
        );

        // Cross-format upstreams report usage after the finish reason: Finish only records the
        // stop reason and closes open blocks, and the terminal delta rides Done carrying the
        // final usage report.
        let finish = renderer.render(&IrEvent::finish(IrFinishReason::Stop));

        assert!(finish.is_empty());

        renderer.render(&IrEvent::Usage(IrUsage {
            input_tokens: 3,
            output_tokens: 6,
            cached_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: Some(2),
        }));

        let terminal = renderer.render(&IrEvent::Done);

        assert_eq!(kinds(&terminal), ["message_delta", "message_stop"]);

        let terminal_payloads = payloads(&terminal);

        assert_eq!(
            at(payload_at(&terminal_payloads, 0), "/delta"),
            &json!({"stop_reason": "end_turn", "stop_sequence": Value::Null}),
        );
        // The terminal delta carries the final report: the reasoning breakdown rides the native
        // `output_tokens_details` spelling.
        assert_eq!(
            at(payload_at(&terminal_payloads, 0), "/usage"),
            &json!({
                "input_tokens": 3,
                "output_tokens": 6,
                "output_tokens_details": {"thinking_tokens": 2},
            }),
        );
    }

    /// Renders a mid-stream failure as the native terminal `error` event through the shared
    /// classification.
    ///
    /// # Panics
    ///
    /// Panics if the rendered frame diverges from the native error shape.
    #[test]
    fn renders_mid_stream_errors_as_the_native_error_event() {
        let mut renderer = SseRenderer::new("m".to_string());

        let frame = renderer.error_frame(&crate::api::Error::InvalidApiKey);

        assert!(frame.starts_with("event: error\n"));

        #[expect(
            clippy::expect_used,
            reason = "the error frame is under test; a malformed frame is the failure"
        )]
        let payload = serde_json::from_str::<Value>(
            frame
                .split_once("data: ")
                .expect("the error frame carries a data line")
                .1,
        )
        .expect("error frame data is rendered JSON");

        assert_eq!(at(&payload, "/type"), &json!("error"));
        assert_eq!(at(&payload, "/error/type"), &json!("authentication_error"));
    }

    /// Pins the failure-path ordering: a failure observed after [`IrEvent::Finish`] renders the
    /// terminal `error` frame alone — no `message_delta` or `message_stop` precedes or follows
    /// it, so a stream that fails after the finish reason terminates on the error event.
    ///
    /// # Panics
    ///
    /// Panics if any frame other than the error frame is rendered around the failure.
    #[test]
    fn renders_only_the_error_frame_for_a_failure_after_finish() {
        let mut renderer = SseRenderer::new("m".to_string());

        renderer.render(&IrEvent::MessageStart {
            id: "up".to_string(),
            model: "up-model".to_string(),
        });

        // The finish resolves the still-buffered head with zeros and closes no block.
        let finish = renderer.render(&IrEvent::finish(IrFinishReason::Stop));
        let failure = vec![renderer.error_frame(&crate::api::Error::Internal)];

        assert_eq!(kinds(&finish), ["message_start"]);
        assert_eq!(kinds(&failure), ["error"]);

        let rendered = [finish, failure].concat();

        assert!(!rendered.iter().any(|frame| frame.contains("message_delta")));
        assert!(!rendered.iter().any(|frame| frame.contains("message_stop")));
    }

    /// Folds the stream-head partial usage report into the opening `message_start`: the
    /// partial report published immediately after [`IrEvent::MessageStart`] (README §6.2)
    /// rides the head's input and cache columns while the stream keeps its native order and
    /// emits the head exactly once, and the consumed partial stands as the last observation
    /// in the terminal delta when no terminal report overwrites it.
    ///
    /// # Panics
    ///
    /// Panics if the rendered stream diverges from the native shape.
    #[test]
    fn folds_the_stream_head_partial_usage_report_into_message_start() {
        let mut renderer = SseRenderer::new("claude-test".to_string());

        // The head waits one event: the start alone renders nothing yet.
        let buffered = renderer.render(&IrEvent::MessageStart {
            id: "up".to_string(),
            model: "up-model".to_string(),
        });

        assert!(buffered.is_empty());

        let start = renderer.render(&IrEvent::Usage(IrUsage {
            input_tokens: 10,
            output_tokens: 0,
            cached_tokens: Some(2),
            cache_write_tokens: Some(3),
            reasoning_tokens: None,
        }));

        assert_eq!(kinds(&start), ["message_start"]);

        // The report's input and cache columns ride the opening usage; the output stays zero
        // until a terminal report.
        assert_eq!(
            at(first_payload(&payloads(&start)), "/message/usage"),
            &json!({
                "input_tokens": 10,
                "output_tokens": 0,
                "cache_creation_input_tokens": 3,
                "cache_read_input_tokens": 2,
            }),
        );

        // The consumed report renders no frame of its own and the head is not repeated.
        let text = renderer.render(&IrEvent::TextDelta("hi".to_string()));

        assert_eq!(kinds(&text), ["content_block_start", "content_block_delta"]);

        // The finish closes the text block the delta opened; the partial stands as the last
        // observation, so the terminal delta carries it.
        let finish = renderer.render(&IrEvent::finish(IrFinishReason::Stop));

        assert_eq!(kinds(&finish), ["content_block_stop"]);

        let terminal = renderer.render(&IrEvent::Done);

        assert_eq!(kinds(&terminal), ["message_delta", "message_stop"]);
        assert_eq!(
            at(payload_at(&payloads(&terminal), 0), "/usage"),
            &json!({
                "input_tokens": 10,
                "output_tokens": 0,
                "cache_creation_input_tokens": 3,
                "cache_read_input_tokens": 2,
            }),
        );
    }

    /// Pins the failure-path ordering when the stream fails while the head is still buffered:
    /// the buffered `message_start` rides ahead of the terminal `error` frame — exactly once
    /// and first — so a stream that fails between the start and its first resolving event
    /// still opens natively.
    ///
    /// # Panics
    ///
    /// Panics if the rendered frames diverge from the native failure shape.
    #[test]
    fn renders_the_buffered_head_before_a_mid_stream_failure() {
        let mut renderer = SseRenderer::new("m".to_string());

        renderer.render(&IrEvent::MessageStart {
            id: "up".to_string(),
            model: "up-model".to_string(),
        });

        let failure = renderer.error_frame(&crate::api::Error::Internal);

        assert!(failure.starts_with("event: message_start\n"));
        assert!(failure.contains("event: error\n"));

        // The head was flushed once: a second failure renders the error frame alone.
        let repeated = renderer.error_frame(&crate::api::Error::Internal);

        assert!(repeated.starts_with("event: error\n"));
        assert!(!repeated.contains("message_start"));
    }
}
