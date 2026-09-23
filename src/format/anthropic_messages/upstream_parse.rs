//! Upstream-event parsing for the Anthropic Messages codec: parses upstream SSE streams or a
//! complete JSON body into IR events.
//!
//! Upstream streams discriminate on the SSE `event:` name — never on the JSON payloads' own `type`
//! members — and parse into the shared IR event stream; mid-stream `error` events abort per the
//! error-mirroring policy.

use super::{TypedBlock, block_opaque};
use crate::{
    format::{PayloadError, UpstreamEventParser},
    ir::{IrEvent, IrFinishReason, IrUsage},
    sse::SseFrame,
};
use serde::Deserialize;
use serde_json::Value;

/// Parses upstream Anthropic Messages responses — event-name-discriminated SSE streams or a
/// complete JSON body — into IR events.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Whether a finish reason was emitted; a clean end-of-stream without one degrades to
    /// [`IrFinishReason::Stop`], so streamed and aggregated renderings agree.
    is_finished: bool,

    /// Input tokens reported by `message_start`, carried into the terminal usage report.
    input_tokens: Option<u64>,

    /// Cache-read input tokens reported by `message_start`.
    cached_tokens: Option<u64>,

    /// Cache-write input tokens reported by `message_start`.
    cache_write_tokens: Option<u64>,

    /// Whether a usage report was emitted; guards the end-of-stream best-effort report.
    is_usage_emitted: bool,

    /// Content blocks opened by `content_block_start`, keyed by stream content index.
    open_blocks: Vec<OpenBlock>,

    /// Number of tool-call blocks opened so far; assigns the dense IR call index.
    tool_count: usize,
}

/// One upstream content block tracked across its start/delta/stop lifetime.
#[derive(Debug)]
struct OpenBlock {
    /// Stream content index naming the block.
    content_index: usize,

    /// Block kind, deciding which delta shapes the block accepts.
    kind: OpenBlockKind,

    /// Dense IR call index, for tool-call blocks only.
    tool_ordinal: Option<usize>,

    /// Invocation identifier revealed by the tool block's start event.
    tool_id: String,

    /// Function name revealed by the tool block's start event.
    tool_name: String,

    /// Argument bytes assembled from `input_json_delta` fragments.
    tool_arguments: String,

    /// Reasoning text assembled from `thinking_delta` fragments.
    thinking_text: String,

    /// Signature delivered by `signature_delta`, terminating the thinking block.
    signature: Option<String>,

    /// Redacted payload carried by the block's start event.
    redacted_data: Option<Value>,
}

/// Kind of a tracked upstream content block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenBlockKind {
    /// Plain text.
    Text,

    /// Reasoning text with a terminating signature.
    Thinking,

    /// Opaque redacted reasoning without deltas.
    Redacted,

    /// Tool invocation with streamed JSON arguments.
    ToolUse,
}

/// Token accounting fragment an upstream usage object carries; every member is optional because
/// `message_start` reports the input columns while `message_delta` reports the output column.
#[derive(Debug, Default, Deserialize)]
struct UpstreamUsage {
    /// Tokens consumed by the input.
    #[serde(default)]
    input_tokens: Option<u64>,

    /// Tokens produced by the output.
    #[serde(default)]
    output_tokens: Option<u64>,

    /// Input tokens served from the upstream prompt cache.
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,

    /// Input tokens written to the upstream prompt cache.
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
}

/// Head of a `message_start` event: the response identity beside its input usage.
#[derive(Debug, Default, Deserialize)]
struct MessageHead {
    /// Upstream response identifier, empty when the upstream omits it.
    #[serde(default)]
    id: String,

    /// Upstream model name, empty when the upstream omits it.
    #[serde(default)]
    model: String,

    /// Input token accounting carried by the start event.
    #[serde(default)]
    usage: Option<UpstreamUsage>,
}

/// The `message_start` event payload.
#[derive(Debug, Deserialize)]
struct MessageStartEvent {
    /// Response head.
    #[serde(default)]
    message: MessageHead,
}

/// A content block as announced by `content_block_start`; unknown block kinds are tolerated and
/// ignored so newer upstream block types never break parsing.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StartBlock {
    /// Plain text block.
    Text,

    /// Reasoning block; its text streams through `thinking_delta`.
    Thinking,

    /// Redacted block carrying its whole payload up front.
    RedactedThinking {
        /// Redacted payload, verbatim from the family.
        #[serde(default)]
        data: Option<Value>,
    },

    /// Tool invocation block revealing its identity up front.
    ToolUse {
        /// Invocation identifier.
        #[serde(default)]
        id: Option<String>,

        /// Invoked function name.
        #[serde(default)]
        name: Option<String>,
    },

    /// Any newer block kind the gateway does not read.
    #[serde(other)]
    Other,
}

/// The `content_block_start` event payload.
#[derive(Debug, Deserialize)]
struct ContentBlockStartEvent {
    /// Stream content index naming the block.
    #[serde(default)]
    index: u64,

    /// Announced block.
    content_block: StartBlock,
}

/// One content delta; unknown delta kinds are tolerated and ignored like unknown blocks.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BlockDelta {
    /// Assistant text fragment.
    #[serde(rename = "text_delta")]
    Text {
        /// Text bytes.
        #[serde(default)]
        text: Option<String>,
    },

    /// Reasoning text fragment.
    #[serde(rename = "thinking_delta")]
    Thinking {
        /// Reasoning bytes.
        #[serde(default)]
        thinking: Option<String>,
    },

    /// Signature terminating a thinking block.
    #[serde(rename = "signature_delta")]
    Signature {
        /// Upstream signature.
        #[serde(default)]
        signature: Option<String>,
    },

    /// Tool argument bytes fragment.
    #[serde(rename = "input_json_delta")]
    InputJson {
        /// JSON bytes.
        #[serde(default)]
        partial_json: Option<String>,
    },

    /// Any newer delta kind the gateway does not read.
    #[serde(other)]
    Other,
}

/// The `content_block_delta` event payload.
#[derive(Debug, Deserialize)]
struct ContentBlockDeltaEvent {
    /// Stream content index naming the block.
    #[serde(default)]
    index: u64,

    /// The delta itself.
    #[serde(default)]
    delta: Option<BlockDelta>,
}

/// The `content_block_stop` event payload.
#[derive(Debug, Deserialize)]
struct ContentBlockStopEvent {
    /// Stream content index naming the block.
    #[serde(default)]
    index: u64,
}

/// Stop-reason head of a `message_delta` event.
#[derive(Debug, Default, Deserialize)]
struct MessageDeltaHead {
    /// Canonical stop reason string, when the turn finished.
    #[serde(default)]
    stop_reason: Option<String>,
}

/// The `message_delta` event payload, carrying the stop reason beside the cumulative output
/// usage.
#[derive(Debug, Default, Deserialize)]
struct MessageDeltaEvent {
    /// Stop-reason head.
    #[serde(default)]
    delta: MessageDeltaHead,

    /// Cumulative output token accounting.
    #[serde(default)]
    usage: Option<UpstreamUsage>,
}

/// Detail of a mid-stream `error` event.
#[derive(Debug, Deserialize)]
struct ErrorDetail {
    /// Human-readable error text, when the event carries one.
    #[serde(default)]
    message: Option<String>,
}

/// The mid-stream `error` event payload, aborting the stream per policy.
#[derive(Debug, Deserialize)]
struct ErrorEvent {
    /// The reported failure.
    #[serde(default)]
    error: Option<ErrorDetail>,
}

/// One content block of a complete non-streaming message; unknown kinds are tolerated and
/// ignored like their streaming counterparts.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseBlock {
    /// Plain text block.
    Text {
        /// The text content.
        #[serde(default)]
        text: Option<String>,
    },

    /// Complete reasoning block with its signature.
    Thinking {
        /// Complete reasoning text.
        #[serde(default)]
        thinking: Option<String>,

        /// Upstream signature terminating the block.
        #[serde(default)]
        signature: Option<String>,
    },

    /// Complete redacted block.
    RedactedThinking {
        /// Redacted payload, verbatim from the family.
        #[serde(default)]
        data: Option<Value>,
    },

    /// Complete tool invocation.
    ToolUse {
        /// Invocation identifier.
        #[serde(default)]
        id: Option<String>,

        /// Invoked function name.
        #[serde(default)]
        name: Option<String>,

        /// JSON arguments object; the fragment's shape is the caller's to define.
        #[serde(default)]
        input: Option<Value>,
    },

    /// Any newer block kind the gateway does not read.
    #[serde(other)]
    Other,
}

/// A complete non-streaming upstream message.
#[derive(Debug, Default, Deserialize)]
struct ResponseMessage {
    /// Upstream response identifier, empty when the upstream omits it.
    #[serde(default)]
    id: String,

    /// Upstream model name, empty when the upstream omits it.
    #[serde(default)]
    model: String,

    /// Content blocks in order.
    #[serde(default)]
    content: Vec<ResponseBlock>,

    /// Canonical stop reason string, when the turn reported one.
    #[serde(default)]
    stop_reason: Option<String>,

    /// Complete token accounting.
    #[serde(default)]
    usage: Option<UpstreamUsage>,
}

impl UpstreamParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Finds the tracked block for a stream content index.
    fn open_block(&mut self, content_index: usize) -> Option<&mut OpenBlock> {
        self.open_blocks
            .iter_mut()
            .find(|open| open.content_index == content_index)
    }

    /// Emits [`IrEvent::MessageStart`] once per response.
    fn message_start_events(&mut self, head: &MessageHead) -> Vec<IrEvent> {
        if self.is_started {
            return Vec::new();
        }

        self.is_started = true;

        vec![IrEvent::MessageStart {
            id: head.id.clone(),
            model: head.model.clone(),
        }]
    }

    /// Records the input usage columns carried by a usage fragment.
    fn record_input_usage(&mut self, usage: &UpstreamUsage) {
        if let Some(input_tokens) = usage.input_tokens {
            self.input_tokens = Some(input_tokens);
        }

        if let Some(cached_tokens) = usage.cache_read_input_tokens {
            self.cached_tokens = Some(cached_tokens);
        }

        if let Some(cache_write_tokens) = usage.cache_creation_input_tokens {
            self.cache_write_tokens = Some(cache_write_tokens);
        }
    }

    /// Renders the terminal usage report from the stored input columns and one output
    /// observation, marking usage emitted.
    fn terminal_usage(&mut self, output_tokens: u64) -> IrEvent {
        self.is_usage_emitted = true;

        IrEvent::Usage(IrUsage {
            input_tokens: self.input_tokens.unwrap_or_default(),
            output_tokens,
            cached_tokens: self.cached_tokens,
            cache_write_tokens: self.cache_write_tokens,
            reasoning_tokens: None,
        })
    }

    /// Opens the block announced by a `content_block_start` event.
    fn ingest_block_start(&mut self, event: ContentBlockStartEvent) -> Vec<IrEvent> {
        let content_index = event.index as usize;

        let (kind, tool_ordinal, tool_id, tool_name, redacted_data) = match event.content_block {
            StartBlock::Text => (
                OpenBlockKind::Text,
                None,
                String::new(),
                String::new(),
                None,
            ),

            StartBlock::Thinking => (
                OpenBlockKind::Thinking,
                None,
                String::new(),
                String::new(),
                None,
            ),

            StartBlock::RedactedThinking { data } => (
                OpenBlockKind::Redacted,
                None,
                String::new(),
                String::new(),
                data,
            ),

            StartBlock::ToolUse { id, name } => {
                let ordinal = self.tool_count;

                self.tool_count += 1;

                (
                    OpenBlockKind::ToolUse,
                    Some(ordinal),
                    id.unwrap_or_default(),
                    name.unwrap_or_default(),
                    None,
                )
            }

            StartBlock::Other => return Vec::new(),
        };

        let mut events = Vec::new();

        // The tool block's start event reveals its identity, opening the call for the streaming
        // client renderers exactly like a leading empty delta does on the OpenAI codecs.
        if let Some(ordinal) = tool_ordinal {
            events.push(IrEvent::ToolCallDelta {
                index: ordinal,
                call_id: Some(tool_id.clone()),
                name: Some(tool_name.clone()),
                arguments_delta: String::new(),
            });
        }

        self.open_blocks.push(OpenBlock {
            content_index,
            kind,
            tool_ordinal,
            tool_id,
            tool_name,
            tool_arguments: String::new(),
            thinking_text: String::new(),
            signature: None,
            redacted_data,
        });

        events
    }

    /// Routes one `content_block_delta` event into IR events through the block tracked for its
    /// index; deltas for untracked indexes are ignored.
    fn ingest_block_delta(&mut self, event: ContentBlockDeltaEvent) -> Vec<IrEvent> {
        let content_index = event.index as usize;

        let Some(delta) = event.delta else {
            return Vec::new();
        };

        match delta {
            BlockDelta::Text { text } => {
                let Some(text) = text.filter(|text| !text.is_empty()) else {
                    return Vec::new();
                };

                if self
                    .open_block(content_index)
                    .is_some_and(|open| open.kind == OpenBlockKind::Text)
                {
                    vec![IrEvent::TextDelta(text)]
                } else {
                    Vec::new()
                }
            }

            BlockDelta::Thinking { thinking } => {
                let Some(thinking) = thinking.filter(|thinking| !thinking.is_empty()) else {
                    return Vec::new();
                };

                let Some(open) = self.open_block(content_index) else {
                    return Vec::new();
                };

                if open.kind != OpenBlockKind::Thinking {
                    return Vec::new();
                }

                open.thinking_text.push_str(&thinking);

                vec![IrEvent::ReasoningDelta(thinking)]
            }

            BlockDelta::Signature { signature } => {
                let Some(open) = self.open_block(content_index) else {
                    return Vec::new();
                };

                if open.kind != OpenBlockKind::Thinking {
                    return Vec::new();
                }

                // The last signature fragment wins; upstreams deliver exactly one.
                if signature.is_some() {
                    open.signature = signature;
                }

                Vec::new()
            }

            BlockDelta::InputJson { partial_json } => {
                let Some(partial_json) =
                    partial_json.filter(|partial_json| !partial_json.is_empty())
                else {
                    return Vec::new();
                };

                let Some(open) = self.open_block(content_index) else {
                    return Vec::new();
                };

                if open.kind != OpenBlockKind::ToolUse {
                    return Vec::new();
                }

                open.tool_arguments.push_str(&partial_json);

                vec![IrEvent::ToolCallDelta {
                    index: open.tool_ordinal.unwrap_or_default(),
                    call_id: None,
                    name: None,
                    arguments_delta: partial_json,
                }]
            }

            BlockDelta::Other => Vec::new(),
        }
    }

    /// Closes the block named by a `content_block_stop` event: thinking blocks terminate with an
    /// empty-text done carrying the streamed signature, tool blocks complete their call, and
    /// text blocks need no closing event.
    fn ingest_block_stop(&mut self, event: ContentBlockStopEvent) -> Vec<IrEvent> {
        let content_index = event.index as usize;

        let position = self
            .open_blocks
            .iter()
            .position(|open| open.content_index == content_index);

        let Some(position) = position else {
            return Vec::new();
        };

        let open = self.open_blocks.swap_remove(position);

        match open.kind {
            OpenBlockKind::Text => Vec::new(),

            OpenBlockKind::Thinking => vec![IrEvent::ReasoningDone {
                text: String::new(),
                signature: open.signature,
                opaque: block_opaque(&TypedBlock::Thinking {
                    thinking: open.thinking_text,
                    signature: None,
                }),
            }],

            // Redacted blocks stream no deltas, so the stop event carries the block's whole
            // content as its opaque replay payload.
            OpenBlockKind::Redacted => vec![IrEvent::ReasoningDone {
                text: String::new(),
                signature: None,
                opaque: block_opaque(&TypedBlock::RedactedThinking {
                    data: open.redacted_data.unwrap_or(Value::Null),
                }),
            }],

            OpenBlockKind::ToolUse => vec![IrEvent::ToolCallDone {
                index: open.tool_ordinal.unwrap_or_default(),
                call_id: open.tool_id,
                name: open.tool_name,
                arguments: open.tool_arguments,
                thought_signature: None,
            }],
        }
    }

    /// Ingests one `message_delta` event: the stop reason finishes the turn, and the output
    /// usage observation joins the stored input columns into the terminal report.
    fn ingest_message_delta(&mut self, event: MessageDeltaEvent) -> Vec<IrEvent> {
        let mut events = Vec::new();

        if let Some(usage) = &event.usage {
            self.record_input_usage(usage);

            events.push(self.terminal_usage(usage.output_tokens.unwrap_or_default()));
        }

        if let Some(stop_reason) = &event.delta.stop_reason {
            self.is_finished = true;

            events.push(IrEvent::Finish(finish_reason_from_stop(stop_reason)));
        }

        events
    }
}

impl UpstreamEventParser for UpstreamParser {
    /// Ingests one upstream SSE frame, discriminated on the frame's `event:` name; the JSON
    /// payloads' own `type` members are never relied on for discrimination. Frames without a
    /// known event name are ignored, so newer upstream event types never break parsing.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for malformed event payloads and for mid-stream `error` events,
    /// which abort the stream per policy.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, PayloadError> {
        let Some(event) = frame.event.as_deref() else {
            return Ok(Vec::new());
        };

        match event {
            "message_start" => {
                let event = crate::format::parse_frame::<MessageStartEvent>(frame)?;

                self.record_input_usage(
                    event
                        .message
                        .usage
                        .as_ref()
                        .unwrap_or(&UpstreamUsage::default()),
                );

                Ok(self.message_start_events(&event.message))
            }

            "content_block_start" => {
                let event = crate::format::parse_frame::<ContentBlockStartEvent>(frame)?;

                Ok(self.ingest_block_start(event))
            }

            "content_block_delta" => {
                let event = crate::format::parse_frame::<ContentBlockDeltaEvent>(frame)?;

                Ok(self.ingest_block_delta(event))
            }

            "content_block_stop" => {
                let event = crate::format::parse_frame::<ContentBlockStopEvent>(frame)?;

                Ok(self.ingest_block_stop(event))
            }

            "message_delta" => {
                let event = crate::format::parse_frame::<MessageDeltaEvent>(frame)?;

                Ok(self.ingest_message_delta(event))
            }

            "message_stop" | "ping" => Ok(Vec::new()),

            "error" => {
                let detail = serde_json::from_str::<ErrorEvent>(&frame.data)
                    .ok()
                    .and_then(|event| event.error)
                    .and_then(|error| error.message)
                    .unwrap_or_else(|| "upstream error event".to_string());

                Err(PayloadError::new(detail))
            }

            _ => Ok(Vec::new()),
        }
    }

    /// Parses a complete upstream non-streaming message into IR events: text rides single
    /// deltas, thinking blocks arrive as full-text dones with no prior deltas, and tool calls
    /// arrive as delta/done pairs — never overlapping, so streaming and aggregated renderings
    /// agree.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for bodies outside the upstream message shape.
    fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, PayloadError> {
        let message = crate::format::parse_body::<ResponseMessage>(value)?;

        let mut events = vec![IrEvent::MessageStart {
            id: message.id,
            model: message.model,
        }];

        self.is_started = true;

        let mut tool_ordinal = 0;

        for block in &message.content {
            match block {
                ResponseBlock::Text { text } => {
                    if text.as_deref().is_some_and(|text| !text.is_empty()) {
                        events.push(IrEvent::TextDelta(text.clone().unwrap_or_default()));
                    }
                }

                ResponseBlock::Thinking {
                    thinking,
                    signature,
                } => {
                    let text = thinking.clone().unwrap_or_default();

                    events.push(IrEvent::ReasoningDone {
                        text: text.clone(),
                        signature: signature.clone(),
                        opaque: block_opaque(&TypedBlock::Thinking {
                            thinking: text,
                            signature: signature.clone(),
                        }),
                    });
                }

                ResponseBlock::RedactedThinking { data } => {
                    events.push(IrEvent::ReasoningDone {
                        text: String::new(),
                        signature: None,
                        opaque: block_opaque(&TypedBlock::RedactedThinking {
                            data: data.clone().unwrap_or(Value::Null),
                        }),
                    });
                }

                ResponseBlock::ToolUse { id, name, input } => {
                    let call_id = id.clone().unwrap_or_default();
                    let name = name.clone().unwrap_or_default();
                    let arguments = input
                        .clone()
                        .map(|input| input_to_arguments(&input))
                        .unwrap_or_default();

                    events.push(IrEvent::ToolCallDelta {
                        index: tool_ordinal,
                        call_id: Some(call_id.clone()),
                        name: Some(name.clone()),
                        arguments_delta: arguments.clone(),
                    });

                    events.push(IrEvent::ToolCallDone {
                        index: tool_ordinal,
                        call_id,
                        name,
                        arguments,
                        thought_signature: None,
                    });

                    tool_ordinal += 1;
                }

                ResponseBlock::Other => {}
            }
        }

        self.is_finished = true;

        events.push(IrEvent::Finish(
            message
                .stop_reason
                .as_deref()
                .map(finish_reason_from_stop)
                .unwrap_or(IrFinishReason::Stop),
        ));

        if let Some(usage) = &message.usage {
            self.record_input_usage(usage);

            events.push(self.terminal_usage(usage.output_tokens.unwrap_or_default()));
        }

        Ok(events)
    }

    /// Ends a cleanly terminated stream: a missing finish degrades to [`IrFinishReason::Stop`],
    /// and a usage-bearing stream that never reported output still publishes its input columns
    /// with a zero output observation.
    fn finish(&mut self) -> Vec<IrEvent> {
        let mut events = Vec::new();

        if !self.is_usage_emitted
            && (self.input_tokens.is_some()
                || self.cached_tokens.is_some()
                || self.cache_write_tokens.is_some())
        {
            events.push(self.terminal_usage(0));
        }

        if !self.is_finished {
            self.is_finished = true;

            events.push(IrEvent::Finish(IrFinishReason::Stop));
        }

        events
    }
}

/// Maps an Anthropic `stop_reason` string onto the canonical reason; unknown values degrade to
/// [`IrFinishReason::Stop`] rather than failing a healthy stream.
fn finish_reason_from_stop(stop_reason: &str) -> IrFinishReason {
    match stop_reason {
        "end_turn" | "stop_sequence" | "pause_turn" => IrFinishReason::Stop,
        "max_tokens" => IrFinishReason::Length,
        "tool_use" => IrFinishReason::ToolCall,
        "refusal" => IrFinishReason::ContentFilter,
        _ => IrFinishReason::Stop,
    }
}
/// Renders a tool `input` fragment as the JSON-encoded arguments text the IR carries.
fn input_to_arguments(input: &Value) -> String {
    crate::format::render_json(input)
}

#[cfg(test)]
mod tests {
    use super::{super::client_render::finish_reason_to_stop, *};
    use serde_json::{Value, json};

    /// Streams one upstream SSE conversation through the event-name-discriminated parser:
    /// `message_start` opens with its input usage, text/thinking/signature/tool deltas assemble
    /// their blocks, `message_delta` finishes with `tool_use` and joins the output usage onto
    /// the stored input columns, and `ping` plus unknown events never surface.
    ///
    /// # Panics
    ///
    /// Panics if the parsed event stream diverges from the documented mapping.
    #[test]
    fn parses_the_streaming_conversation_by_event_name() {
        let mut parser = UpstreamParser::new();

        let frames = |event: &str, data: Value| SseFrame {
            data: data.to_string(),
            event: Some(event.to_string()),
        };

        let mut events = Vec::new();

        for frame in [
            frames(
                "message_start",
                json!({"message": {"id": "msg_1", "model": "claude-test",
                                   "usage": {"input_tokens": 10,
                                             "cache_read_input_tokens": 2,
                                             "cache_creation_input_tokens": 3}}}),
            ),
            frames("ping", json!({})),
            frames(
                "content_block_start",
                json!({"index": 0, "content_block": {"type": "text"}}),
            ),
            frames(
                "content_block_delta",
                json!({"index": 0, "delta": {"type": "text_delta", "text": "hi"}}),
            ),
            frames("content_block_stop", json!({"index": 0})),
            frames(
                "content_block_start",
                json!({"index": 1, "content_block": {"type": "thinking"}}),
            ),
            frames(
                "content_block_delta",
                json!({"index": 1, "delta": {"type": "thinking_delta",
                                              "thinking": "draft"}}),
            ),
            frames(
                "content_block_delta",
                json!({"index": 1, "delta": {"type": "signature_delta",
                                              "signature": "sig"}}),
            ),
            frames("content_block_stop", json!({"index": 1})),
            frames(
                "content_block_start",
                json!({"index": 2, "content_block": {"type": "tool_use",
                                                     "id": "call_1",
                                                     "name": "get_weather"}}),
            ),
            frames(
                "content_block_delta",
                json!({"index": 2, "delta": {"type": "input_json_delta",
                                              "partial_json": "{\"city\":"}}),
            ),
            frames(
                "content_block_delta",
                json!({"index": 2, "delta": {"type": "input_json_delta",
                                              "partial_json": "\"Paris\"}"}}),
            ),
            frames("content_block_stop", json!({"index": 2})),
            frames(
                "message_delta",
                json!({"delta": {"stop_reason": "tool_use", "stop_sequence": Value::Null},
                       "usage": {"output_tokens": 25}}),
            ),
            frames("message_stop", json!({})),
            frames("custom_future_event", json!({"anything": true})),
        ] {
            #[expect(
                clippy::expect_used,
                reason = "the fixture stream is constructed to parse"
            )]
            let ingested = parser
                .ingest_frame(&frame)
                .expect("every fixture frame parses");

            events.extend(ingested);
        }

        assert_eq!(
            events,
            vec![
                IrEvent::MessageStart {
                    id: "msg_1".to_string(),
                    model: "claude-test".to_string(),
                },
                IrEvent::TextDelta("hi".to_string()),
                IrEvent::ReasoningDelta("draft".to_string()),
                IrEvent::ReasoningDone {
                    text: String::new(),
                    signature: Some("sig".to_string()),
                    opaque: json!({
                        "type": "thinking",
                        "thinking": "draft",
                    }),
                },
                IrEvent::ToolCallDelta {
                    index: 0,
                    call_id: Some("call_1".to_string()),
                    name: Some("get_weather".to_string()),
                    arguments_delta: String::new(),
                },
                IrEvent::ToolCallDelta {
                    index: 0,
                    call_id: None,
                    name: None,
                    arguments_delta: "{\"city\":".to_string(),
                },
                IrEvent::ToolCallDelta {
                    index: 0,
                    call_id: None,
                    name: None,
                    arguments_delta: "\"Paris\"}".to_string(),
                },
                IrEvent::ToolCallDone {
                    index: 0,
                    call_id: "call_1".to_string(),
                    name: "get_weather".to_string(),
                    arguments: "{\"city\":\"Paris\"}".to_string(),
                    thought_signature: None,
                },
                IrEvent::Usage(IrUsage {
                    input_tokens: 10,
                    output_tokens: 25,
                    cached_tokens: Some(2),
                    cache_write_tokens: Some(3),
                    reasoning_tokens: None,
                }),
                IrEvent::Finish(IrFinishReason::ToolCall),
            ],
        );

        // A clean end after the terminal events emits nothing further.
        assert!(parser.finish().is_empty());
    }

    /// Maps every documented `stop_reason` onto its finish reason, degrading unknown values to
    /// `Stop`; a mid-stream `error` event aborts, and a usage-bearing stream that ends without
    /// an output report still publishes its input columns.
    ///
    /// # Panics
    ///
    /// Panics if any mapping, abort, or terminal report diverges from the contract.
    #[test]
    fn maps_stop_reasons_aborts_on_error_events_and_reports_terminal_usage() {
        for (stop_reason, expected) in [
            ("end_turn", IrFinishReason::Stop),
            ("max_tokens", IrFinishReason::Length),
            ("tool_use", IrFinishReason::ToolCall),
            ("stop_sequence", IrFinishReason::Stop),
            ("refusal", IrFinishReason::ContentFilter),
            ("pause_turn", IrFinishReason::Stop),
            ("future_reason", IrFinishReason::Stop),
        ] {
            assert_eq!(finish_reason_from_stop(stop_reason), expected);
            assert_eq!(
                finish_reason_to_stop(expected),
                match expected {
                    IrFinishReason::Stop => "end_turn",
                    IrFinishReason::Length => "max_tokens",
                    IrFinishReason::ToolCall => "tool_use",
                    IrFinishReason::ContentFilter => "refusal",
                },
            );
        }

        let mut parser = UpstreamParser::new();

        let error_frame = SseFrame {
            data: json!({"error": {"type": "overloaded_error",
                                   "message": "busy"}})
            .to_string(),
            event: Some("error".to_string()),
        };

        #[expect(
            clippy::expect_used,
            reason = "the error event is constructed to abort"
        )]
        let failure = parser
            .ingest_frame(&error_frame)
            .expect_err("a mid-stream error aborts");

        assert_eq!(failure.to_string(), "busy");

        // Frames without an event name carry no discriminable event and never surface.
        let mut parser = UpstreamParser::new();

        #[expect(clippy::expect_used, reason = "an eventless frame parses trivially")]
        let ignored = parser
            .ingest_frame(&SseFrame {
                data: json!({"type": "message_start"}).to_string(),
                event: None,
            })
            .expect("an eventless frame is ignored");

        assert!(ignored.is_empty());

        // Input usage without a later output report still publishes at end-of-stream.
        #[expect(
            clippy::expect_used,
            reason = "the start frame is constructed to parse"
        )]
        parser
            .ingest_frame(&SseFrame {
                data: json!({"message": {"id": "m", "model": "c",
                                         "usage": {"input_tokens": 7}}})
                .to_string(),
                event: Some("message_start".to_string()),
            })
            .expect("the start frame parses");

        assert_eq!(
            parser.finish(),
            vec![
                IrEvent::Usage(IrUsage {
                    input_tokens: 7,
                    output_tokens: 0,
                    cached_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                }),
                IrEvent::Finish(IrFinishReason::Stop),
            ],
        );
    }

    /// Parses a complete non-streaming message: text rides one delta, the thinking block
    /// arrives as a full-text done with no prior deltas, the tool call as a delta/done pair,
    /// and the usage joins every reported column with no reasoning breakdown.
    ///
    /// # Panics
    ///
    /// Panics if the parsed events diverge from the documented mapping.
    #[test]
    fn parses_the_complete_non_streaming_message() {
        let mut parser = UpstreamParser::new();

        let body = json!({
            "id": "msg_2",
            "model": "claude-test",
            "content": [
                {"type": "text", "text": "done"},
                {"type": "thinking", "thinking": "full", "signature": "sig"},
                {"type": "redacted_thinking", "data": "enc"},
                {"type": "tool_use", "id": "call_9", "name": "search",
                 "input": {"q": "x"}},
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 5, "output_tokens": 9,
                      "cache_read_input_tokens": 1,
                      "cache_creation_input_tokens": 2},
        });

        #[expect(
            clippy::expect_used,
            reason = "the fixture body is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body)
            .expect("the complete message parses");

        assert_eq!(
            events,
            vec![
                IrEvent::MessageStart {
                    id: "msg_2".to_string(),
                    model: "claude-test".to_string(),
                },
                IrEvent::TextDelta("done".to_string()),
                IrEvent::ReasoningDone {
                    text: "full".to_string(),
                    signature: Some("sig".to_string()),
                    opaque: json!({
                        "type": "thinking",
                        "thinking": "full",
                        "signature": "sig",
                    }),
                },
                IrEvent::ReasoningDone {
                    text: String::new(),
                    signature: None,
                    opaque: json!({"type": "redacted_thinking", "data": "enc"}),
                },
                IrEvent::ToolCallDelta {
                    index: 0,
                    call_id: Some("call_9".to_string()),
                    name: Some("search".to_string()),
                    arguments_delta: r#"{"q":"x"}"#.to_string(),
                },
                IrEvent::ToolCallDone {
                    index: 0,
                    call_id: "call_9".to_string(),
                    name: "search".to_string(),
                    arguments: r#"{"q":"x"}"#.to_string(),
                    thought_signature: None,
                },
                IrEvent::Finish(IrFinishReason::Stop),
                IrEvent::Usage(IrUsage {
                    input_tokens: 5,
                    output_tokens: 9,
                    cached_tokens: Some(1),
                    cache_write_tokens: Some(2),
                    reasoning_tokens: None,
                }),
            ],
        );
    }
}
