//! Upstream-event parsing for the Anthropic Messages codec: parses upstream SSE streams or a
//! complete JSON body into IR events.
//!
//! Upstream streams discriminate on the SSE `event:` name — never on the JSON payloads' own `type`
//! members — and parse into the shared IR event stream; mid-stream `error` events abort per the
//! error-mirroring policy.

use super::{TypedBlock, block_opaque, compaction::CompactionBlock};
use crate::{
    format::{PayloadError, UpstreamEventParser, message_start_once},
    ir::{IrEvent, IrFinishReason, IrUsage},
    sse::SseFrame,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Parses upstream Anthropic Messages responses — event-name-discriminated SSE streams or a
/// complete JSON body — into IR events.
///
/// Compaction facts ride beside the IR event stream: every `compaction` content block is
/// recorded verbatim in [`UpstreamParser::compaction_blocks`], a `usage.iterations` array in
/// [`UpstreamParser::compaction_iterations`], and the `compaction` stop reason in
/// [`UpstreamParser::compaction_stop`]. State and metadata events carry these facts
/// through aggregation, client rendering, and accounting.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Whether a finish reason was emitted; `finish` degrades a stream that ends without one
    /// through [`crate::format::degrade_finish_once`].
    is_finished: bool,

    /// Input tokens reported by `message_start`, carried into the terminal usage report.
    input_tokens: Option<u64>,

    /// Cache-read input tokens reported by `message_start`.
    cached_tokens: Option<u64>,

    /// Cache-write input tokens reported by `message_start`.
    cache_write_tokens: Option<u64>,

    /// Output tokens spent on extended thinking, reported by the usage breakdown.
    reasoning_tokens: Option<u64>,

    /// Whether a usage report was emitted; guards the end-of-stream best-effort report.
    is_usage_emitted: bool,

    /// Content blocks opened by `content_block_start`, keyed by stream content index.
    open_blocks: Vec<OpenBlock>,

    /// Incomplete threshold blocks awaiting their full summary delta and stop. A block whose
    /// stop never arrives stays withheld at a clean end-of-stream — the stop is the compaction
    /// state's completeness boundary (the block authorizes history replacement, so an
    /// unterminated one is never state) — and is dropped rather than flushed.
    pending_compaction_blocks: Vec<(usize, Value)>,

    /// Number of tool-call blocks opened so far; assigns the dense IR call index.
    tool_count: usize,

    /// Every `compaction` content block observed, in order: its stream content index (on a
    /// complete body, its position in the content array) and the block's raw vendor JSON. No IR
    /// events are emitted for these blocks; the compaction outward render owns their lifecycles.
    pub(crate) compaction_blocks: Vec<(usize, Value)>,

    /// The last `usage.iterations` array the upstream reported, verbatim: one entry per
    /// sampling iteration a native compaction performed. Last observation wins, like every
    /// other usage column; the compaction outward render decides the entries' shape.
    pub(crate) compaction_iterations: Vec<crate::ir::compaction::UsageIteration>,

    /// Whether the upstream reported the `compaction` stop reason. The canonical finish
    /// vocabulary has no compaction member, so the parser emits the ordinary `Stop` finish and
    /// records the fact beside it; the compaction outward render overrides the stop reason for
    /// compaction lifecycles.
    pub(crate) compaction_stop: bool,
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

    /// Redacted payload string carried by the block's start event.
    redacted_data: Option<String>,
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

    /// Output token breakdown, absent when the upstream reports totals only — in streaming it
    /// rides only the terminal `message_delta` event.
    #[serde(default)]
    output_tokens_details: Option<OutputTokensDetails>,

    /// Raw vendor iteration entries: one entry per sampling iteration a native compaction
    /// performed, captured verbatim for the compaction outward render.
    #[serde(default)]
    iterations: Option<Vec<crate::ir::compaction::UsageIteration>>,
}

/// The upstream output token breakdown.
#[derive(Debug, Default, Deserialize)]
struct OutputTokensDetails {
    /// Output tokens spent on extended thinking.
    #[serde(default)]
    thinking_tokens: Option<u64>,
}

impl UpstreamUsage {
    /// Whether the fragment carries any input-side column — the columns a native
    /// `message_start` reports for the input the turn already consumed.
    fn is_input_reported(&self) -> bool {
        self.input_tokens.is_some()
            || self.cache_read_input_tokens.is_some()
            || self.cache_creation_input_tokens.is_some()
    }
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

/// A typed ordinary block or a complete producer-owned compaction fragment.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum CapturedBlock<T> {
    /// Ordinary protocol fields consumed by the IR translator.
    Ordinary(T),
    /// Complete native compaction state, including unmodeled metadata.
    Compaction(Value),
}

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for CapturedBlock<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// Selects the block vocabulary without interpreting foreign compaction members.
        #[derive(Deserialize)]
        struct Kind {
            /// Required block discriminator.
            #[serde(rename = "type")]
            kind: String,
        }
        let raw = <&serde_json::value::RawValue>::deserialize(deserializer)?;
        let kind: Kind = serde_json::from_str(raw.get()).map_err(serde::de::Error::custom)?;
        if kind.kind == "compaction" {
            serde_json::from_str(raw.get())
                .map(Self::Compaction)
                .map_err(serde::de::Error::custom)
        } else {
            serde_json::from_str(raw.get())
                .map(Self::Ordinary)
                .map_err(serde::de::Error::custom)
        }
    }
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
        /// Redacted payload string, verbatim from the family; absent or null carries none, and
        /// any other JSON kind contradicts the protocol and fails the frame (README §6.2).
        #[serde(default)]
        data: Option<String>,
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

    /// The announced block, captured raw: compaction blocks replay verbatim, so the typed view
    /// parses beside the untouched vendor JSON.
    content_block: CapturedBlock<StartBlock>,
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

    /// The complete threshold-compaction summary, delivered in one delta.
    #[serde(rename = "compaction_delta")]
    Compaction {
        /// Full readable summary, or a native no-op.
        content: Option<String>,
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
        /// Redacted payload string, verbatim from the family; absent or null carries none, and
        /// any other JSON kind contradicts the protocol and fails the body (README §6.2).
        #[serde(default)]
        data: Option<String>,
    },

    /// Complete tool invocation.
    ToolUse {
        /// Invocation identifier.
        #[serde(default)]
        id: Option<String>,

        /// Invoked function name.
        #[serde(default)]
        name: Option<String>,

        /// JSON arguments object; absent or null is the accepted empty-arguments form, and any
        /// other JSON kind contradicts the protocol and fails the body (README §6.2).
        #[serde(default)]
        input: Option<Map<String, Value>>,
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

    /// Content blocks in order, kept raw: compaction blocks replay verbatim, so the typed view
    /// parses per block beside the untouched vendor JSON.
    #[serde(default)]
    content: Vec<CapturedBlock<ResponseBlock>>,

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
        message_start_once(&mut self.is_started, head.id.clone(), head.model.clone())
    }

    /// Emits the stream-opening [`IrEvent::MessageStart`] with empty identity on the recovery
    /// paths that never saw the head announcement: every event stream opens with it exactly
    /// once, first (README §6.2).
    fn ensure_open(&mut self) -> Vec<IrEvent> {
        message_start_once(&mut self.is_started, String::new(), String::new())
    }

    /// Records the cumulative usage columns carried by a usage fragment: the input columns on
    /// `message_start`, and the output-side thinking breakdown on the terminal `message_delta`
    /// or complete body. Every reported column is kept; absent ones leave the stored value.
    fn record_usage(&mut self, usage: &UpstreamUsage) {
        if let Some(input_tokens) = usage.input_tokens {
            self.input_tokens = Some(input_tokens);
        }

        if let Some(cached_tokens) = usage.cache_read_input_tokens {
            self.cached_tokens = Some(cached_tokens);
        }

        if let Some(cache_write_tokens) = usage.cache_creation_input_tokens {
            self.cache_write_tokens = Some(cache_write_tokens);
        }

        if let Some(thinking_tokens) = usage
            .output_tokens_details
            .as_ref()
            .and_then(|details| details.thinking_tokens)
        {
            self.reasoning_tokens = Some(thinking_tokens);
        }

        // Last observation wins, like every other usage column.
        if let Some(iterations) = &usage.iterations {
            self.compaction_iterations = iterations.clone();
        }
    }

    /// Renders one usage report from the stored columns and one output observation, marking a
    /// report as published: the `message_start` partial opens with a zero output observation,
    /// the terminal `message_delta` or complete body carries the cumulative one, and every
    /// consumer — renderer and accounting alike — takes the last observation.
    fn usage_event(&mut self, output_tokens: u64) -> IrEvent {
        self.is_usage_emitted = true;

        IrEvent::Usage(IrUsage {
            input_tokens: self.input_tokens.unwrap_or_default(),
            output_tokens,
            cached_tokens: self.cached_tokens,
            cache_write_tokens: self.cache_write_tokens,
            reasoning_tokens: self.reasoning_tokens,
        })
    }

    /// Opens the block announced by a `content_block_start` event.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] when the announced block does not parse.
    fn ingest_block_start(
        &mut self,
        event: ContentBlockStartEvent,
    ) -> Result<Vec<IrEvent>, PayloadError> {
        let content_index = event.index as usize;

        let block = match event.content_block {
            CapturedBlock::Ordinary(block) => block,
            CapturedBlock::Compaction(raw) => {
                let view = CompactionBlock::deserialize(&raw)
                    .map_err(|_| PayloadError::new("malformed upstream payload in SSE frame"))?;
                if view.content.is_some() {
                    self.compaction_blocks.push((content_index, raw.clone()));
                    return Ok(vec![IrEvent::CompactionState { payload: raw }]);
                }
                self.pending_compaction_blocks.push((content_index, raw));
                return Ok(Vec::new());
            }
        };

        let (kind, tool_ordinal, tool_id, tool_name, redacted_data) = match block {
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

            // The compaction case returned above; `serde(other)` covers the rest.
            StartBlock::Other => {
                return Ok(Vec::new());
            }
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

        Ok(events)
    }

    /// Routes one `content_block_delta` event into IR events through the block tracked for its
    /// index; deltas for untracked indexes are ignored.
    ///
    /// # Panics
    ///
    /// Panics if a concrete vendor compaction fragment cannot serialize.
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

                #[expect(
                    clippy::expect_used,
                    reason = "the ToolUse-kind guard above proves the block carries the ordinal \
                              its start event assigned"
                )]
                let ordinal = open.tool_ordinal.expect("tool blocks carry their ordinal");

                vec![IrEvent::ToolCallDelta {
                    index: ordinal,
                    call_id: None,
                    name: None,
                    arguments_delta: partial_json,
                }]
            }

            BlockDelta::Compaction { content } => {
                #[derive(serde::Serialize)]
                struct CompletedBlock {
                    /// The original vendor fragment, with optional-field presence retained.
                    #[serde(flatten)]
                    original: Value,
                    /// The complete summary replaces the opening null content.
                    content: Option<String>,
                }
                if let Some((_, block)) = self
                    .pending_compaction_blocks
                    .iter_mut()
                    .find(|(index, _)| *index == content_index)
                {
                    #[expect(clippy::expect_used, reason = "a concrete vendor fragment serializes")]
                    {
                        *block = serde_json::to_value(CompletedBlock {
                            original: block.clone(),
                            content,
                        })
                        .expect("the completed block serializes");
                    }
                }
                Vec::new()
            }
            BlockDelta::Other => Vec::new(),
        }
    }

    /// Closes the block named by a `content_block_stop` event: a pending compaction block whose
    /// summary has arrived closes as its state event, and ordinary blocks close through
    /// [`UpstreamParser::close_block`].
    fn ingest_block_stop(&mut self, event: ContentBlockStopEvent) -> Vec<IrEvent> {
        let content_index = event.index as usize;
        if let Some(position) = self
            .pending_compaction_blocks
            .iter()
            .position(|(index, _)| *index == content_index)
        {
            let (index, block) = self.pending_compaction_blocks.remove(position);
            self.compaction_blocks.push((index, block.clone()));
            return vec![IrEvent::CompactionState { payload: block }];
        }

        let position = self
            .open_blocks
            .iter()
            .position(|open| open.content_index == content_index);

        let Some(position) = position else {
            return Vec::new();
        };

        Self::close_block(self.open_blocks.swap_remove(position))
    }

    /// Closes a tracked block into its closing IR events: thinking blocks terminate with an
    /// empty-text done carrying the streamed signature, whose opaque replay payload includes the
    /// signature exactly like the complete-body path, tool blocks complete their call, and text
    /// blocks need no closing event.
    ///
    /// # Panics
    ///
    /// Panics if a ToolUse-kind tracked block carries no call ordinal — a first-party parser
    /// invariant: the start event assigns one to every tool block it opens.
    fn close_block(open: OpenBlock) -> Vec<IrEvent> {
        match open.kind {
            OpenBlockKind::Text => Vec::new(),

            OpenBlockKind::Thinking => {
                // The signature rides the opaque payload, not the separate done field alone:
                // consumers render the opaque verbatim, and a thinking block replayed to
                // Anthropic without its signature is rejected with a 400 (README §6.1).
                let signature = open.signature;

                vec![IrEvent::ReasoningDone {
                    text: String::new(),
                    signature: signature.clone(),
                    opaque: block_opaque(&TypedBlock::Thinking {
                        thinking: open.thinking_text,
                        signature,
                    }),
                }]
            }

            // Redacted blocks stream no deltas, so the stop event carries the block's whole
            // content as its opaque replay payload.
            OpenBlockKind::Redacted => vec![IrEvent::ReasoningDone {
                text: String::new(),
                signature: None,
                opaque: block_opaque(&TypedBlock::RedactedThinking {
                    data: open.redacted_data.map(Value::String).unwrap_or(Value::Null),
                }),
            }],

            OpenBlockKind::ToolUse => {
                #[expect(
                    clippy::expect_used,
                    reason = "the match arm proves the block carries the ordinal its start \
                              event assigned"
                )]
                let ordinal = open.tool_ordinal.expect("tool blocks carry their ordinal");

                vec![IrEvent::ToolCallDone {
                    index: ordinal,
                    call_id: open.tool_id,
                    name: open.tool_name,
                    arguments: open.tool_arguments,
                    thought_signature: None,
                }]
            }
        }
    }

    /// Ingests one `message_delta` event: the stop reason finishes the turn, and the output
    /// usage observation joins the stored input columns into the terminal report.
    fn ingest_message_delta(&mut self, event: MessageDeltaEvent) -> Vec<IrEvent> {
        let mut events = Vec::new();

        if let Some(usage) = &event.usage {
            self.record_usage(usage);

            events.push(self.usage_event(usage.output_tokens.unwrap_or_default()));
        }

        if let Some(stop_reason) = &event.delta.stop_reason {
            self.is_finished = true;

            // The compaction stop reason is recorded beside the canonical finish: the closed
            // vocabulary has no compaction member, and the compaction outward render overrides
            // the stop reason for compaction lifecycles.
            if stop_reason == "compaction" {
                self.compaction_stop = true;
            }

            events.push(IrEvent::finish(finish_reason_from_stop(stop_reason)));
        }

        if self.compaction_stop || !self.compaction_iterations.is_empty() {
            events.push(IrEvent::CompactionMetadata {
                is_paused: self.compaction_stop,
                iterations: self.compaction_iterations.clone(),
            });
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

                let usage = event.message.usage.as_ref();

                self.record_usage(usage.unwrap_or(&UpstreamUsage::default()));

                let mut events = self.message_start_events(&event.message);

                // Native streams report the true input columns here, so the partial report is
                // published immediately and consumers observe real input usage from the stream
                // head; the terminal report still arrives later and overwrites it — last
                // observation wins.
                if usage.is_some_and(UpstreamUsage::is_input_reported) {
                    events.push(self.usage_event(0));
                }

                if !self.compaction_iterations.is_empty() {
                    events.push(IrEvent::CompactionMetadata {
                        is_paused: false,
                        iterations: self.compaction_iterations.clone(),
                    });
                }
                Ok(events)
            }

            "content_block_start" => {
                let event = crate::format::parse_frame::<ContentBlockStartEvent>(frame)?;

                // A stream opening at its first content block never saw the head announcement;
                // the recovery opens the event stream with the empty identity.
                let mut events = self.ensure_open();

                events.extend(self.ingest_block_start(event)?);

                Ok(events)
            }

            "content_block_delta" => {
                let event = crate::format::parse_frame::<ContentBlockDeltaEvent>(frame)?;

                let mut events = self.ensure_open();

                events.extend(self.ingest_block_delta(event));

                Ok(events)
            }

            "content_block_stop" => {
                let event = crate::format::parse_frame::<ContentBlockStopEvent>(frame)?;

                let mut events = self.ensure_open();

                events.extend(self.ingest_block_stop(event));

                Ok(events)
            }

            "message_delta" => {
                let event = crate::format::parse_frame::<MessageDeltaEvent>(frame)?;

                let mut events = self.ensure_open();

                events.extend(self.ingest_message_delta(event));

                Ok(events)
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
    fn parse_json_response(&mut self, body: &[u8]) -> Result<Vec<IrEvent>, PayloadError> {
        let message = crate::format::parse_body::<ResponseMessage>(body)?;

        let mut events = vec![IrEvent::MessageStart {
            id: message.id,
            model: message.model,
        }];

        self.is_started = true;

        let mut tool_ordinal = 0;

        for (content_index, captured) in message.content.iter().enumerate() {
            let block = match captured {
                CapturedBlock::Ordinary(block) => block,
                CapturedBlock::Compaction(raw) => {
                    CompactionBlock::deserialize(raw)
                        .map_err(|_| PayloadError::new("malformed upstream response body"))?;
                    self.compaction_blocks.push((content_index, raw.clone()));
                    events.push(IrEvent::CompactionState {
                        payload: raw.clone(),
                    });
                    continue;
                }
            };

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
                            data: data.clone().map(Value::String).unwrap_or(Value::Null),
                        }),
                    });
                }

                ResponseBlock::ToolUse { id, name, input } => {
                    let call_id = id.clone().unwrap_or_default();
                    let name = name.clone().unwrap_or_default();
                    let arguments = input.as_ref().map(input_to_arguments).unwrap_or_default();

                    events.extend(crate::format::complete_call_events(
                        tool_ordinal,
                        call_id,
                        name,
                        arguments,
                        None,
                    ));

                    tool_ordinal += 1;
                }

                ResponseBlock::Other => {}
            }
        }

        self.is_finished = true;

        // The compaction stop reason is recorded beside the canonical finish: the closed
        // vocabulary has no compaction member, and the compaction outward render overrides the
        // stop reason for compaction lifecycles.
        if message.stop_reason.as_deref() == Some("compaction") {
            self.compaction_stop = true;
        }

        events.push(IrEvent::finish(
            message
                .stop_reason
                .as_deref()
                .map(finish_reason_from_stop)
                .unwrap_or(IrFinishReason::Stop),
        ));

        if let Some(usage) = &message.usage {
            self.record_usage(usage);

            events.push(self.usage_event(usage.output_tokens.unwrap_or_default()));
        }

        if self.compaction_stop || !self.compaction_iterations.is_empty() {
            events.push(IrEvent::CompactionMetadata {
                is_paused: self.compaction_stop,
                iterations: self.compaction_iterations.clone(),
            });
        }
        Ok(events)
    }

    /// Ends a cleanly terminated stream: the buffered cross-frame block state flushes before
    /// the terminal events — every open block closes exactly as its `content_block_stop` would,
    /// so an upstream that ended cleanly without the closing frames still delivers the content
    /// it streamed (README §6.3) — a usage-bearing stream that never published a report still
    /// publishes its columns with a zero output observation, and a stream that ends without a
    /// finish degrades through [`crate::format::degrade_finish_once`]. Pending compaction
    /// blocks are the exception: their stop is the completeness boundary of native compaction
    /// state, so an unterminated one stays withheld.
    fn finish(&mut self) -> Vec<IrEvent> {
        let mut events = Vec::new();

        if !self.open_blocks.is_empty() {
            let mut open_blocks = std::mem::take(&mut self.open_blocks);

            // `swap_remove` scrambles the tracked order during ingestion; the content order is
            // the stream order the closing events must follow.
            open_blocks.sort_by_key(|open| open.content_index);

            events.extend(open_blocks.into_iter().flat_map(Self::close_block));
        }

        if !self.is_usage_emitted
            && (self.input_tokens.is_some()
                || self.cached_tokens.is_some()
                || self.cache_write_tokens.is_some()
                || self.reasoning_tokens.is_some())
        {
            events.push(self.usage_event(0));
        }

        events.extend(crate::format::degrade_finish_once(&mut self.is_finished));

        events
    }
}

/// Maps an Anthropic `stop_reason` string onto the canonical reason; unknown values degrade to
/// [`IrFinishReason::Stop`] rather than failing a healthy stream.
fn finish_reason_from_stop(stop_reason: &str) -> IrFinishReason {
    match stop_reason {
        "end_turn" | "stop_sequence" | "pause_turn" => IrFinishReason::Stop,
        // Both truncation reasons — an output cap and a turn cut off because the context
        // window filled — map onto the canonical truncation finish, so clients can
        // distinguish either from a clean stop and resubmit a shortened history instead of
        // looping on an over-context one.
        "max_tokens" | "model_context_window_exceeded" => IrFinishReason::Length,
        "tool_use" => IrFinishReason::ToolCall,
        "refusal" => IrFinishReason::ContentFilter,
        // The compaction stop reason keeps the canonical Stop finish here: the closed
        // vocabulary has no compaction member, the parser records the fact in
        // `compaction_stop`, and the compaction outward render overrides the stop reason for
        // compaction lifecycles.
        "compaction" => IrFinishReason::Stop,
        _ => IrFinishReason::Stop,
    }
}
/// Maps a tool `input` object onto the JSON-encoded arguments text the IR carries.
fn input_to_arguments(input: &Map<String, Value>) -> String {
    crate::format::render_json(input)
}

#[cfg(test)]
mod tests {
    use super::{
        super::client_render::{SseRenderer, finish_reason_to_stop, render_json_response},
        *,
    };
    use crate::{
        format::{ClientSseRenderer, aggregated_body_json},
        ir::Aggregator,
    };
    use serde_json::{Value, json};

    /// Streams one upstream SSE conversation through the event-name-discriminated parser:
    /// `message_start` opens with its input usage and publishes the partial input-side report
    /// immediately after the start, text/thinking/signature/tool deltas assemble their blocks,
    /// `message_delta` finishes with `tool_use` and joins the output usage and its
    /// `output_tokens_details.thinking_tokens` breakdown onto the stored columns, and `ping`
    /// plus unknown events never surface.
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
                       "usage": {"output_tokens": 25,
                                 "output_tokens_details": {"thinking_tokens": 7}}}),
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
                IrEvent::Usage(IrUsage {
                    input_tokens: 10,
                    output_tokens: 0,
                    cached_tokens: Some(2),
                    cache_write_tokens: Some(3),
                    reasoning_tokens: None,
                }),
                IrEvent::TextDelta("hi".to_string()),
                IrEvent::ReasoningDelta("draft".to_string()),
                IrEvent::ReasoningDone {
                    text: String::new(),
                    signature: Some("sig".to_string()),
                    opaque: json!({
                        "type": "thinking",
                        "thinking": "draft",
                        "signature": "sig",
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
                    reasoning_tokens: Some(7),
                }),
                IrEvent::finish(IrFinishReason::ToolCall),
            ],
        );

        // A clean end after the terminal events emits nothing further.
        assert!(parser.finish().is_empty());
    }

    /// Streams a thinking turn and verifies both outward renderings agree on the block: the
    /// aggregated replay renders the streamed block's opaque payload verbatim — the streamed
    /// signature included, exactly as the complete-body path builds it — and the streamed render
    /// delivers the same text and signature through its deltas, so either rendering hands back a
    /// block Anthropic accepts unchanged (README §6.1).
    ///
    /// # Panics
    ///
    /// Panics if either rendering diverges from the replayed block.
    #[test]
    fn the_streamed_thinking_block_replays_with_its_signature() {
        let mut parser = UpstreamParser::new();

        let frames = |event: &str, data: Value| SseFrame {
            data: data.to_string(),
            event: Some(event.to_string()),
        };

        let mut events = Vec::new();

        for frame in [
            frames(
                "message_start",
                json!({"message": {"id": "msg_1", "model": "claude-test"}}),
            ),
            frames(
                "content_block_start",
                json!({"index": 0, "content_block": {"type": "thinking"}}),
            ),
            frames(
                "content_block_delta",
                json!({"index": 0, "delta": {"type": "thinking_delta",
                                              "thinking": "draft"}}),
            ),
            frames(
                "content_block_delta",
                json!({"index": 0, "delta": {"type": "signature_delta",
                                              "signature": "sig"}}),
            ),
            frames("content_block_stop", json!({"index": 0})),
            frames(
                "message_delta",
                json!({"delta": {"stop_reason": "end_turn"},
                       "usage": {"output_tokens": 3}}),
            ),
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

        // The aggregated replay renders the block verbatim, signature included.
        let mut aggregator = Aggregator::default();

        for event in &events {
            aggregator.ingest(event);
        }

        let aggregation = aggregator.finish();

        let block = json!({
            "type": "thinking",
            "thinking": "draft",
            "signature": "sig",
        });

        assert_eq!(
            aggregation
                .reasoning_blocks
                .first()
                .map(|block| &block.opaque),
            Some(&block),
        );

        let body = aggregated_body_json(&render_json_response("claude-test", &aggregation));

        assert_eq!(body.pointer("/content/0"), Some(&block));

        // The streamed render delivers the same block through its deltas: the reasoning text
        // followed by the closing signature.
        let mut renderer = SseRenderer::new("claude-test".to_string());

        let mut rendered = Vec::new();

        for event in &events {
            rendered.extend(renderer.render(event));
        }

        let deltas = rendered
            .iter()
            .filter(|frame| frame.starts_with("event: content_block_delta"))
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
            .collect::<Vec<_>>();

        assert_eq!(
            deltas,
            vec![
                json!({"type": "content_block_delta", "index": 0,
                       "delta": {"type": "thinking_delta", "thinking": "draft"}}),
                json!({"type": "content_block_delta", "index": 0,
                       "delta": {"type": "signature_delta", "signature": "sig"}}),
            ],
        );
    }

    /// Maps every documented `stop_reason` onto its finish reason, degrading unknown values to
    /// `Stop`; a mid-stream `error` event aborts, and input-only usage publishes as the partial
    /// report at the start, leaving the end-of-stream fallback nothing to publish.
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
            ("model_context_window_exceeded", IrFinishReason::Length),
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

        // Input usage publishes immediately as the partial report at the stream head; with no
        // later output report arriving, the end-of-stream fallback has nothing left to publish.
        #[expect(
            clippy::expect_used,
            reason = "the start frame is constructed to parse"
        )]
        let events = parser
            .ingest_frame(&SseFrame {
                data: json!({"message": {"id": "m", "model": "c",
                                         "usage": {"input_tokens": 7}}})
                .to_string(),
                event: Some("message_start".to_string()),
            })
            .expect("the start frame parses");

        assert_eq!(
            events,
            vec![
                IrEvent::MessageStart {
                    id: "m".to_string(),
                    model: "c".to_string(),
                },
                IrEvent::Usage(IrUsage {
                    input_tokens: 7,
                    output_tokens: 0,
                    cached_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                }),
            ],
        );

        assert_eq!(parser.finish(), vec![IrEvent::finish(IrFinishReason::Stop)]);
    }

    /// Publishes the partial input-side usage report immediately after [`IrEvent::MessageStart`]
    /// when the upstream `message_start` event carries input or cache columns, and emits no
    /// usage report when it does not — neither for an absent usage object nor for one without
    /// input columns.
    ///
    /// # Panics
    ///
    /// Panics if any published event sequence diverges from the partial-report contract.
    #[test]
    fn publishes_partial_input_usage_at_message_start_only_for_input_columns() {
        let frames = |event: &str, data: Value| SseFrame {
            data: data.to_string(),
            event: Some(event.to_string()),
        };

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the start frame is constructed to parse"
        )]
        let events = parser
            .ingest_frame(&frames(
                "message_start",
                json!({"message": {"id": "msg_1", "model": "claude-test",
                                   "usage": {"input_tokens": 12,
                                             "cache_read_input_tokens": 4,
                                             "cache_creation_input_tokens": 2}}}),
            ))
            .expect("the start frame parses");

        assert_eq!(
            events,
            vec![
                IrEvent::MessageStart {
                    id: "msg_1".to_string(),
                    model: "claude-test".to_string(),
                },
                IrEvent::Usage(IrUsage {
                    input_tokens: 12,
                    output_tokens: 0,
                    cached_tokens: Some(4),
                    cache_write_tokens: Some(2),
                    reasoning_tokens: None,
                }),
            ],
        );

        // A `message_start` without a usage object carries no report: only the start surfaces,
        // and the clean end-of-stream fallback has no columns to publish either.
        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the start frame is constructed to parse"
        )]
        let events = parser
            .ingest_frame(&frames(
                "message_start",
                json!({"message": {"id": "m", "model": "c"}}),
            ))
            .expect("the start frame parses");

        assert_eq!(
            events,
            vec![IrEvent::MessageStart {
                id: "m".to_string(),
                model: "c".to_string(),
            }],
        );

        assert_eq!(parser.finish(), vec![IrEvent::finish(IrFinishReason::Stop)]);

        // A usage object reporting output totals only carries no input-side column and is not
        // an input report.
        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the start frame is constructed to parse"
        )]
        let events = parser
            .ingest_frame(&frames(
                "message_start",
                json!({"message": {"id": "m", "model": "c",
                                   "usage": {"output_tokens": 1}}}),
            ))
            .expect("the start frame parses");

        assert_eq!(
            events,
            vec![IrEvent::MessageStart {
                id: "m".to_string(),
                model: "c".to_string(),
            }],
        );
    }

    /// Folds a complete stream whose `message_start` carried input usage through the aggregation
    /// and the streamed Anthropic rendering: the terminal report wins everywhere — the partial
    /// report never surfaces as the final usage, and it renders no terminal delta of its own.
    ///
    /// # Panics
    ///
    /// Panics if any final usage diverges from the terminal report.
    #[test]
    fn the_terminal_usage_report_overrides_the_partial_one() {
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
                "message_delta",
                json!({"delta": {"stop_reason": "end_turn"},
                       "usage": {"output_tokens": 25}}),
            ),
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

        let terminal = IrUsage {
            input_tokens: 10,
            output_tokens: 25,
            cached_tokens: Some(2),
            cache_write_tokens: Some(3),
            reasoning_tokens: None,
        };

        // The partial report is published first, the cumulative one last.
        let usage_events: Vec<IrUsage> = events
            .iter()
            .filter_map(|event| match event {
                IrEvent::Usage(usage) => Some(*usage),
                _ => None,
            })
            .collect();

        assert_eq!(
            usage_events,
            vec![
                IrUsage {
                    input_tokens: 10,
                    output_tokens: 0,
                    cached_tokens: Some(2),
                    cache_write_tokens: Some(3),
                    reasoning_tokens: None,
                },
                terminal,
            ],
        );

        // The aggregation ends on the terminal report, so the aggregated rendering and the
        // accounting it feeds never see the partial one.
        let mut aggregator = Aggregator::default();

        for event in &events {
            aggregator.ingest(event);
        }

        assert_eq!(aggregator.finish().usage, Some(terminal));

        // The streamed rendering opens on the partial report and closes its single terminal
        // delta on the terminal one.
        let mut renderer = SseRenderer::new("claude-test".to_string());

        let mut rendered = Vec::new();

        for event in &events {
            rendered.extend(renderer.render(event));
        }

        rendered.extend(renderer.render(&IrEvent::Done));

        let payloads = |frames: &[String]| {
            frames
                .iter()
                .map(|frame| {
                    (
                        frame
                            .lines()
                            .next()
                            .unwrap_or_default()
                            .trim_start_matches("event: ")
                            .to_string(),
                        #[expect(
                            clippy::expect_used,
                            reason = "the rendered frames are under test; a malformed frame is \
                                      the failure"
                        )]
                        serde_json::from_str::<Value>(
                            frame
                                .split_once("data: ")
                                .expect("every frame carries a data line")
                                .1,
                        )
                        .expect("frame data is rendered JSON"),
                    )
                })
                .collect::<Vec<_>>()
        };

        let payloads = payloads(&rendered);

        assert_eq!(
            payloads
                .iter()
                .filter(|(kind, _)| kind == "message_delta")
                .count(),
            1,
        );

        #[expect(
            clippy::expect_used,
            reason = "the terminal delta is under test; its absence is the failure"
        )]
        let delta = payloads
            .iter()
            .find(|(kind, _)| kind == "message_delta")
            .expect("the terminal delta renders");

        assert_eq!(
            delta.1.pointer("/usage"),
            Some(&json!({
                "input_tokens": 10,
                "output_tokens": 25,
                "cache_creation_input_tokens": 3,
                "cache_read_input_tokens": 2,
            })),
        );
    }

    /// Parses a complete non-streaming message: text rides one delta, the thinking block
    /// arrives as a full-text done with no prior deltas, the tool call as a delta/done pair,
    /// and the usage joins every reported column, the `output_tokens_details.thinking_tokens`
    /// breakdown included.
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
                      "cache_creation_input_tokens": 2,
                      "output_tokens_details": {"thinking_tokens": 4}},
        });

        #[expect(
            clippy::expect_used,
            reason = "the fixture body is constructed to parse"
        )]
        let events = parser
            .parse_json_response(&crate::format::test_json_body(&(body)))
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
                IrEvent::finish(IrFinishReason::Stop),
                IrEvent::Usage(IrUsage {
                    input_tokens: 5,
                    output_tokens: 9,
                    cached_tokens: Some(1),
                    cache_write_tokens: Some(2),
                    reasoning_tokens: Some(4),
                }),
            ],
        );
    }

    /// Captures a compaction block of a complete non-streaming message verbatim — beside the
    /// ordinary blocks' IR events — records the `compaction` stop reason beside its canonical
    /// `Stop` finish, and captures the raw `usage.iterations` entries.
    ///
    /// # Panics
    ///
    /// Panics if the captured facts or the event stream diverge from the documented mapping.
    #[test]
    fn captures_the_compaction_facts_of_a_complete_body() {
        let mut parser = UpstreamParser::new();

        let block = json!({
            "type": "compaction",
            "content": "the summary",
            "signature": "sig",
            "encrypted_content": "enc",
            "tool_changes": [{"type": "tool_addition"}],
            "vendor_extra_member": {"kept": true},
        });

        let iterations = vec![
            json!({
                "type": "compaction",
                "input_tokens": 180_000,
                "output_tokens": 3_500,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
            }),
            json!({"type": "message", "input_tokens": 23_000, "output_tokens": 1_000}),
        ];

        let body = json!({
            "id": "msg_3",
            "model": "claude-test",
            "content": [
                block.clone(),
                {"type": "text", "text": "answer"},
            ],
            "stop_reason": "compaction",
            "usage": {"input_tokens": 23_000, "output_tokens": 1_000,
                      "iterations": iterations.clone()},
        });

        #[expect(
            clippy::expect_used,
            reason = "the fixture body is constructed to parse"
        )]
        let events = parser
            .parse_json_response(&crate::format::test_json_body(&(body)))
            .expect("the complete message parses");

        // The compaction block recorded verbatim, including vendor-owned extra members; the
        // text block still rode its ordinary IR event.
        assert_eq!(parser.compaction_blocks, vec![(0, block.clone())]);
        assert!(parser.compaction_stop);
        assert_eq!(
            crate::format::aggregated_body_json(&parser.compaction_iterations),
            serde_json::json!(iterations)
        );

        assert_eq!(
            events,
            vec![
                IrEvent::MessageStart {
                    id: "msg_3".to_string(),
                    model: "claude-test".to_string(),
                },
                IrEvent::CompactionState {
                    payload: block.clone()
                },
                IrEvent::TextDelta("answer".to_string()),
                IrEvent::finish(IrFinishReason::Stop),
                IrEvent::Usage(IrUsage {
                    input_tokens: 23_000,
                    output_tokens: 1_000,
                    cached_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                }),
                IrEvent::CompactionMetadata {
                    is_paused: true,
                    iterations: iterations.clone().into_iter().map(Into::into).collect()
                },
            ]
        );

        // A null-content compaction block captures the same way.
        let mut parser = UpstreamParser::new();

        let body = json!({
            "id": "msg_4",
            "model": "claude-test",
            "content": [{"type": "compaction"}],
            "stop_reason": "max_tokens",
        });

        #[expect(
            clippy::expect_used,
            reason = "the null-block body is constructed to parse"
        )]
        let events = parser
            .parse_json_response(&crate::format::test_json_body(&(body)))
            .expect("the null-block message parses");

        assert_eq!(
            parser.compaction_blocks,
            vec![(0, json!({"type": "compaction"}))]
        );
        assert!(!parser.compaction_stop);
        assert!(parser.compaction_iterations.is_empty());
        assert_eq!(
            events.first(),
            Some(&IrEvent::MessageStart {
                id: "msg_4".to_string(),
                model: "claude-test".to_string(),
            })
        );
    }

    /// Rejects third-party members whose JSON kind contradicts the protocol — a non-object
    /// `tool_use.input` and a non-string redacted `data` — as a malformed payload instead of
    /// silently degrading them (README §6.2), while the absent, null, and official forms still
    /// parse in both the streaming and complete-body directions.
    ///
    /// # Panics
    ///
    /// Panics if any probe parses or rejects against the contract.
    #[test]
    fn rejects_kind_contradictions_in_tool_input_and_redacted_data() {
        let parse_body = |content: Value| {
            let mut parser = UpstreamParser::new();

            parser.parse_json_response(&crate::format::test_json_body(&json!({
                "id": "msg_k",
                "model": "claude-test",
                "content": [content],
                "stop_reason": "end_turn",
            })))
        };

        // A non-object `input` — array, string, or number alike — fails the body.
        for input in [json!([1, 2]), json!("args"), json!(3)] {
            #[expect(clippy::expect_used, reason = "the rejection is the tested contract")]
            let failure = parse_body(json!({
                "type": "tool_use",
                "id": "call_1",
                "name": "search",
                "input": input,
            }))
            .expect_err("a non-object tool input fails the body");

            assert!(
                failure
                    .to_string()
                    .starts_with("malformed upstream payload")
            );
        }

        // The absent and null forms are the accepted empty-arguments case and the object form
        // is the official one: all three parse, the first two with empty arguments.
        for (input, arguments) in [
            (
                json!({"type": "tool_use", "id": "call_1", "name": "search"}),
                "",
            ),
            (
                json!({"type": "tool_use", "id": "call_1", "name": "search",
                       "input": Value::Null}),
                "",
            ),
            (
                json!({"type": "tool_use", "id": "call_1", "name": "search",
                       "input": {"q": "x"}}),
                r#"{"q":"x"}"#,
            ),
        ] {
            #[expect(clippy::expect_used, reason = "the acceptance is the tested contract")]
            let events = parse_body(input).expect("the accepted tool input forms parse the body");

            assert_eq!(
                events.iter().find_map(|event| match event {
                    IrEvent::ToolCallDone { arguments, .. } => Some(arguments.as_str()),
                    _ => None,
                }),
                Some(arguments),
            );
        }

        // A non-string redacted `data` fails both directions...
        #[expect(clippy::expect_used, reason = "the rejection is the tested contract")]
        let failure = parse_body(json!({"type": "redacted_thinking", "data": 5}))
            .expect_err("a non-string redacted data fails the body");

        assert!(
            failure
                .to_string()
                .starts_with("malformed upstream payload")
        );

        let mut parser = UpstreamParser::new();

        let frames = |event: &str, data: Value| SseFrame {
            data: data.to_string(),
            event: Some(event.to_string()),
        };

        #[expect(clippy::expect_used, reason = "the rejection is the tested contract")]
        parser
            .ingest_frame(&frames(
                "content_block_start",
                json!({"index": 0, "content_block": {"type": "redacted_thinking",
                                                     "data": 5}}),
            ))
            .expect_err("a non-string redacted data fails the start frame");

        // ...while the string form rides through and the absent form carries none. Each probe
        // parser's first frame is a content-block start, so the events open with the recovered
        // head.
        #[expect(clippy::expect_used, reason = "the acceptance is the tested contract")]
        let events = parser
            .ingest_frame(&frames(
                "content_block_start",
                json!({"index": 0, "content_block": {"type": "redacted_thinking",
                                                     "data": "enc"}}),
            ))
            .expect("the string redacted data parses");

        assert_eq!(
            events,
            vec![IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            }],
        );

        #[expect(
            clippy::expect_used,
            reason = "the fixture frame is constructed to parse"
        )]
        let events = parser
            .ingest_frame(&frames("content_block_stop", json!({"index": 0})))
            .expect("the stop frame parses");

        assert_eq!(
            events,
            vec![IrEvent::ReasoningDone {
                text: String::new(),
                signature: None,
                opaque: json!({"type": "redacted_thinking", "data": "enc"}),
            }],
        );

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the fixture frame is constructed to parse"
        )]
        let events = parser
            .ingest_frame(&frames(
                "content_block_start",
                json!({"index": 0, "content_block": {"type": "redacted_thinking"}}),
            ))
            .expect("the absent redacted data parses");

        assert_eq!(
            events,
            vec![IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            }],
        );

        #[expect(
            clippy::expect_used,
            reason = "the fixture frame is constructed to parse"
        )]
        let events = parser
            .ingest_frame(&frames("content_block_stop", json!({"index": 0})))
            .expect("the stop frame parses");

        assert_eq!(
            events,
            vec![IrEvent::ReasoningDone {
                text: String::new(),
                signature: None,
                opaque: json!({"type": "redacted_thinking", "data": Value::Null}),
            }],
        );
    }

    /// Recovers a stream whose head `message_start` never arrived: the first content-block or
    /// terminal-delta frame opens the event stream with the empty-identity
    /// [`IrEvent::MessageStart`], exactly once and first, and a late head event is a no-op
    /// (README §6.2).
    ///
    /// # Panics
    ///
    /// Panics if the recovered event streams diverge from the open-exactly-once contract.
    #[test]
    fn recovers_a_missing_stream_head() {
        let frames = |event: &str, data: Value| SseFrame {
            data: data.to_string(),
            event: Some(event.to_string()),
        };

        // The first content-block frame opens the stream with the empty identity.
        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the fixture frame is constructed to parse"
        )]
        let events = parser
            .ingest_frame(&frames(
                "content_block_start",
                json!({"index": 0, "content_block": {"type": "text"}}),
            ))
            .expect("the frame parses");

        assert_eq!(
            events,
            vec![IrEvent::MessageStart {
                id: String::new(),
                model: String::new(),
            }],
        );

        // A late head event is a no-op, and a delta for the untracked index stays ignored.
        #[expect(
            clippy::expect_used,
            reason = "the fixture frame is constructed to parse"
        )]
        let events = parser
            .ingest_frame(&frames(
                "message_start",
                json!({"message": {"id": "msg_late", "model": "claude-test"}}),
            ))
            .expect("the late head parses");

        assert!(events.is_empty());

        // A terminal delta on a head-less stream opens the stream first, then rides its usage
        // and finish as usual.
        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the fixture frame is constructed to parse"
        )]
        let events = parser
            .ingest_frame(&frames(
                "message_delta",
                json!({"delta": {"stop_reason": "end_turn"},
                       "usage": {"output_tokens": 4}}),
            ))
            .expect("the frame parses");

        assert_eq!(
            events,
            vec![
                IrEvent::MessageStart {
                    id: String::new(),
                    model: String::new(),
                },
                IrEvent::Usage(IrUsage {
                    input_tokens: 0,
                    output_tokens: 4,
                    cached_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                }),
                IrEvent::finish(IrFinishReason::Stop),
            ],
        );
    }

    /// Flushes the buffered cross-frame state at a clean end-of-stream (README §6.3): an open
    /// thinking block closes with its signature and opaque replay payload, an open tool call
    /// completes, and an open text block needs nothing — all in content order, before the
    /// degraded finish. A pending compaction block whose summary delta arrived but whose stop
    /// never did stays withheld: the stop is the compaction state's completeness boundary, so
    /// the unterminated block neither escapes nor joins the recorded blocks.
    ///
    /// # Panics
    ///
    /// Panics if the flushed event stream diverges from the flush contract.
    #[test]
    fn flushes_buffered_block_state_at_a_clean_end_of_stream() {
        let mut parser = UpstreamParser::new();

        let frames = |event: &str, data: Value| SseFrame {
            data: data.to_string(),
            event: Some(event.to_string()),
        };

        for frame in [
            frames(
                "content_block_start",
                json!({"index": 0, "content_block": {"type": "compaction",
                                                     "signature": "sig"}}),
            ),
            frames(
                "content_block_delta",
                json!({"index": 0, "delta": {"type": "compaction_delta",
                                             "content": "summary"}}),
            ),
            frames(
                "content_block_start",
                json!({"index": 1, "content_block": {"type": "text"}}),
            ),
            frames(
                "content_block_delta",
                json!({"index": 1, "delta": {"type": "text_delta", "text": "hi"}}),
            ),
            frames(
                "content_block_start",
                json!({"index": 2, "content_block": {"type": "thinking"}}),
            ),
            frames(
                "content_block_delta",
                json!({"index": 2, "delta": {"type": "thinking_delta",
                                             "thinking": "draft"}}),
            ),
            frames(
                "content_block_delta",
                json!({"index": 2, "delta": {"type": "signature_delta",
                                             "signature": "sig"}}),
            ),
            frames(
                "content_block_start",
                json!({"index": 3, "content_block": {"type": "tool_use",
                                                     "id": "call_1",
                                                     "name": "search"}}),
            ),
            frames(
                "content_block_delta",
                json!({"index": 3, "delta": {"type": "input_json_delta",
                                             "partial_json": "{\"q\":\"x\"}"}}),
            ),
        ] {
            #[expect(clippy::expect_used, reason = "the fixture frames parse")]
            parser
                .ingest_frame(&frame)
                .expect("every fixture frame parses");
        }

        // The head-less stream opened with the recovered MessageStart; every ordinary block is
        // still open at the clean end-of-stream, so `finish` closes them, while the
        // unterminated compaction block stays withheld.
        assert_eq!(
            parser.finish(),
            vec![
                IrEvent::ReasoningDone {
                    text: String::new(),
                    signature: Some("sig".to_string()),
                    opaque: json!({
                        "type": "thinking",
                        "thinking": "draft",
                        "signature": "sig",
                    }),
                },
                IrEvent::ToolCallDone {
                    index: 0,
                    call_id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: "{\"q\":\"x\"}".to_string(),
                    thought_signature: None,
                },
                IrEvent::finish(IrFinishReason::Stop),
            ],
        );

        assert!(parser.compaction_blocks.is_empty());
    }

    /// Captures a compaction block announced by a streaming `content_block_start` verbatim —
    /// emitting a state event — lets ordinary blocks continue after it, and records the
    /// `compaction` stop reason from the terminal `message_delta`.
    ///
    /// # Panics
    ///
    /// Panics if the captured facts or the event stream diverge from the documented mapping.
    #[test]
    fn captures_the_compaction_block_of_a_stream() {
        let mut parser = UpstreamParser::new();

        let frames = |event: &str, data: Value| SseFrame {
            data: data.to_string(),
            event: Some(event.to_string()),
        };

        let block = json!({
            "type": "compaction",
            "content": "full summary",
            "signature": "sig",
        });

        let mut events = Vec::new();

        for frame in [
            frames(
                "message_start",
                json!({"message": {"id": "msg_5", "model": "claude-test",
                                   "usage": {"input_tokens": 0, "output_tokens": 0,
                                             "iterations": [{"type": "compaction",
                                                             "input_tokens": 9,
                                                             "output_tokens": 4}]}}}),
            ),
            frames(
                "content_block_start",
                json!({"index": 0, "content_block": block.clone()}),
            ),
            frames(
                "content_block_start",
                json!({"index": 1, "content_block": {"type": "text"}}),
            ),
            frames(
                "content_block_delta",
                json!({"index": 1, "delta": {"type": "text_delta", "text": "answer"}}),
            ),
            frames("content_block_stop", json!({"index": 1})),
            frames(
                "message_delta",
                json!({"delta": {"stop_reason": "compaction"},
                       "usage": {"output_tokens": 4}}),
            ),
            frames("message_stop", json!({})),
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

        assert_eq!(parser.compaction_blocks, vec![(0, block.clone())]);
        assert!(parser.compaction_stop);
        assert_eq!(
            crate::format::aggregated_body_json(&parser.compaction_iterations),
            json!([{"type": "compaction", "input_tokens": 9, "output_tokens": 4}])
        );

        // The ordinary text block still delivered its event; the compaction block rode the
        // event stream verbatim at its block position, and the finish degraded to the
        // canonical Stop.
        assert_eq!(
            events,
            vec![
                IrEvent::MessageStart {
                    id: "msg_5".to_string(),
                    model: "claude-test".to_string(),
                },
                IrEvent::Usage(IrUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cached_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                }),
                IrEvent::CompactionMetadata {
                    is_paused: false,
                    iterations: parser.compaction_iterations.clone()
                },
                IrEvent::CompactionState {
                    payload: block.clone()
                },
                IrEvent::TextDelta("answer".to_string()),
                IrEvent::Usage(IrUsage {
                    input_tokens: 0,
                    output_tokens: 4,
                    cached_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                }),
                IrEvent::finish(IrFinishReason::Stop),
                IrEvent::CompactionMetadata {
                    is_paused: true,
                    iterations: parser.compaction_iterations.clone()
                },
            ]
        );
    }
}
