//! Anthropic Messages codec: the `anthropic_messages` upstream format and the inbound
//! `/api/anthropic-v1/messages` client surface (REFACTOR §§1, 2, 4, 5, 6; README §§5.5, 6.4–6.6).
//!
//! Upstream requests render `POST {base}/v1/messages` (`x-api-key`, pinned `anthropic-version`
//! headers are attached by the gateway): the `system` string from IR instructions, IR messages
//! grouped into strictly alternating user/assistant turns, assistant reasoning blocks before their
//! tool calls, `tool_result` blocks carrying `is_error`, typed function tools, the four-mode
//! `tool_choice`, verbatim sampling parameters, `max_tokens` from the client's budget else the
//! synthesis constant, and `output_config.effort` verbatim — or `thinking: disabled` for `none`
//! (README §6.4). Upstream streams discriminate on the SSE `event:` name — never on the JSON
//! payloads' own `type` members — and parse into the shared IR event stream; mid-stream `error`
//! events abort per the error-mirroring policy.
//!
//! The inbound surface parses `POST /messages` bodies strictly into IR (`max_tokens` required per
//! the spec, streaming via body `stream: true`) and renders completions back in the native
//! Messages shape, streamed (`message_start`, `content_block_*`, `message_delta`, `message_stop`)
//! or aggregated. Same-family thinking/redacted blocks round-trip byte-exact through the verbatim
//! `opaque` replay payload; cross-family the opaque payload is dropped.

use crate::{
    api_logs::ApiLogEndpointType,
    format::{
        ClientFormat, ClientSseRenderer, ParsedRequest, PayloadError, RouteContext, StringOrArray,
        UpstreamEventParser, error_anthropic,
    },
    ids,
    ir::{
        InvalidRequest, IrAggregation, IrEvent, IrFinishReason, IrImage, IrMessage, IrPart,
        IrReasoningLevel, IrRequest, IrResponseFormat, IrRole, IrTool, IrToolChoice, IrUsage,
    },
    sse::{self, SseFrame},
};
use serde::{Deserialize, Serialize, de::IgnoredAny};
use serde_json::Value;

/// Fixed synthesis constant rendered as `max_tokens` when the client sent no
/// `max_output_tokens`: the API requires the field, so the gateway documents this stand-in
/// rather than inventing a caller-specific budget (REFACTOR §6).
pub(crate) const SYNTHESIZED_MAX_TOKENS: u64 = 16384;

// ---------------------------------------------------------------------------
// Upstream request rendering
// ---------------------------------------------------------------------------

/// The upstream Anthropic Messages request payload.
#[derive(Debug, Serialize)]
pub(crate) struct RequestBody<'a> {
    /// Upstream model name, rendered verbatim from the resolved row.
    model: &'a str,

    /// Output token budget: the client's `max_output_tokens`, else
    /// [`SYNTHESIZED_MAX_TOKENS`].
    max_tokens: u64,

    /// Concatenated system instructions, omitted when the client sent none.
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,

    /// Strictly alternating user/assistant turns, first turn user.
    messages: Vec<UpstreamMessage>,

    /// Advertised function tools, omitted when the request has none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<UpstreamTool<'a>>,

    /// Tool selection restriction, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<UpstreamToolChoice<'a>>,

    /// Sampling temperature, passed through verbatim; the upstream is the sole authority on
    /// support (Claude 4.7+ rejects non-default values, mirrored per policy).
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,

    /// Nucleus sampling threshold, passed through verbatim like `temperature`.
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,

    /// Whether the upstream streams its response.
    stream: bool,

    /// Reasoning level rendered verbatim; omitted when the client sent no effort and when the
    /// effort is `none` (which renders `thinking: disabled` instead).
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<OutputConfig<'a>>,

    /// Native off spelling, rendered only for the `none` effort level (README §6.4).
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingDisabled>,
}

/// One upstream conversation turn.
#[derive(Debug, Serialize)]
struct UpstreamMessage {
    /// Turn side: Anthropic knows only user and assistant turns.
    role: UpstreamRole,

    /// Content blocks of the turn, never empty (empty rendered content is not emitted).
    content: Vec<OutBlock>,
}

/// Turn side of an upstream message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum UpstreamRole {
    /// The human side, also carrying tool results.
    User,

    /// The model side, carrying text, thinking, and tool calls.
    Assistant,
}

/// One Anthropic content block in the outward direction, covering both the upstream request and
/// the client response shapes, which share the block vocabulary.
///
/// [`OutBlock::Verbatim`] carries a same-family opaque replay payload — an inbound
/// thinking/redacted block stored verbatim — so the gateway replays it without modification;
/// [`OutBlock::Typed`] carries everything the gateway itself constructs.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum OutBlock {
    /// A block the gateway constructed from IR parts.
    Typed(TypedBlock),

    /// A same-family block replayed verbatim from its stored opaque payload: the fragment's
    /// shape is the family's to define, so it travels as a [`Value`].
    Verbatim(Value),
}

/// A content block the gateway constructed.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TypedBlock {
    /// Plain text.
    Text {
        /// The text content.
        text: String,
    },

    /// Image content.
    Image {
        /// Inline data or remote URL source.
        source: ImageSourceOut,
    },

    /// Assistant tool invocation.
    ToolUse {
        /// Identifier of the invocation, round-tripped to the matching result.
        id: String,

        /// Invoked function name.
        name: String,

        /// JSON-encoded arguments object.
        input: Value,
    },

    /// Tool result inside a user turn.
    ToolResult {
        /// Identifier of the invocation being answered.
        tool_use_id: String,

        /// Result content, rendered as a plain string.
        content: String,

        /// Error flag, carried only when the tool reported an error.
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },

    /// Assistant reasoning text with its upstream signature.
    Thinking {
        /// Reasoning text.
        thinking: String,

        /// Upstream signature terminating the block, when the family exposes one.
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// Opaque redacted reasoning the gateway cannot read.
    RedactedThinking {
        /// Redacted payload, verbatim from the family.
        data: Value,
    },
}

/// Image source in the outward direction.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ImageSourceOut {
    /// Base64-encoded inline data.
    Base64 {
        /// MIME type of the image data.
        media_type: String,

        /// Base64-encoded image bytes.
        data: String,
    },

    /// Remote image referenced by URL.
    Url {
        /// Remote `http`/`https` URL.
        url: String,
    },
}

/// One advertised function tool in the upstream shape.
#[derive(Debug, Serialize)]
struct UpstreamTool<'a> {
    /// Function name the model calls.
    name: &'a str,

    /// Human-readable description, omitted when the tool has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,

    /// JSON Schema of the accepted arguments; the client's schema rides through, and a tool
    /// declared without one receives the empty object schema.
    input_schema: Value,
}

/// A `tool_choice` restriction in the upstream shape.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UpstreamToolChoice<'a> {
    /// The model decides freely.
    Auto,

    /// The model must call a tool.
    Any,

    /// The model must call the named function.
    Tool {
        /// Required function name.
        name: &'a str,
    },

    /// The model may not call tools.
    None,
}

/// Reasoning level container, carrying the shared vocabulary value verbatim.
#[derive(Debug, Serialize)]
struct OutputConfig<'a> {
    /// Reasoning effort in the closed shared vocabulary, written verbatim.
    effort: &'a str,
}

/// Native off spelling of the thinking control.
#[derive(Debug, Serialize)]
struct ThinkingDisabled {
    /// Always `disabled`.
    #[serde(rename = "type")]
    kind: &'static str,
}

/// Default input schema for a tool declared without parameters: the empty object schema, modelled
/// as a concrete type rather than assembled JSON.
#[derive(Debug, Serialize)]
struct DefaultInputSchema {
    /// Always `object`.
    #[serde(rename = "type")]
    kind: &'static str,
}

/// Renders the default input schema value.
///
/// # Panics
///
/// Panics if the first-party schema type fails to serialize, which cannot occur.
fn default_input_schema() -> Value {
    #[expect(
        clippy::expect_used,
        reason = "the default schema is a first-party type whose serialization is total"
    )]
    serde_json::to_value(&DefaultInputSchema { kind: "object" })
        .expect("first-party wire types serialize")
}

/// Serializes a constructed block into its opaque replay value: under strict inbound parsing the
/// typed struct already captured the block completely, so the value is the block verbatim.
///
/// # Panics
///
/// Panics if the first-party block type fails to serialize, which cannot occur.
fn block_opaque(block: &TypedBlock) -> Value {
    #[expect(
        clippy::expect_used,
        reason = "constructed blocks are first-party types whose serialization is total"
    )]
    serde_json::to_value(block).expect("first-party wire types serialize")
}

/// Renders the upstream request URL and payload for an Anthropic Messages upstream:
/// `POST {base}/v1/messages` (README §5.5).
///
/// # Errors
///
/// Returns [`InvalidRequest`] when the IR carries a control the upstream has no slot for — tool
/// arguments that do not encode a JSON object, or a response format stricter than plain text.
pub(crate) fn render_request<'a>(
    ir: &'a IrRequest,
    base_url: &str,
    is_streaming: bool,
) -> Result<(String, RequestBody<'a>), InvalidRequest> {
    if let Some(response_format) = &ir.response_format {
        match response_format {
            // Plain text is the absence of a constraint, so it renders as nothing.
            IrResponseFormat::Text => {}

            // The Messages API has no response-format slot; dropping a caller-requested JSON
            // constraint would silently return prose, so the mismatch fails explicitly instead.
            IrResponseFormat::Json | IrResponseFormat::JsonSchema { .. } => {
                return Err(InvalidRequest::field(
                    "response_format",
                    "The Anthropic Messages upstream has no response format control",
                ));
            }
        }
    }

    let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));

    let tools = ir
        .tools
        .iter()
        .map(|tool| UpstreamTool {
            name: &tool.name,
            description: tool.description.as_deref(),
            input_schema: tool.parameters.clone().unwrap_or_else(default_input_schema),
        })
        .collect();

    let tool_choice = ir
        .tool_choice
        .as_ref()
        .map(|tool_choice| match tool_choice {
            IrToolChoice::Auto => UpstreamToolChoice::Auto,
            IrToolChoice::None => UpstreamToolChoice::None,
            IrToolChoice::Required => UpstreamToolChoice::Any,
            IrToolChoice::Function { name } => UpstreamToolChoice::Tool { name },
        });

    let (output_config, thinking) = match &ir.reasoning_effort {
        None => (None, None),

        Some(IrReasoningLevel::None) => (None, Some(ThinkingDisabled { kind: "disabled" })),

        Some(level) => (
            Some(OutputConfig {
                effort: level.as_str(),
            }),
            None,
        ),
    };

    Ok((
        url,
        RequestBody {
            model: &ir.model,
            max_tokens: ir.max_output_tokens.unwrap_or(SYNTHESIZED_MAX_TOKENS),
            system: ir.instructions.as_deref(),
            messages: render_messages(ir)?,
            tools,
            tool_choice,
            temperature: ir.temperature,
            top_p: ir.top_p,
            stream: is_streaming,
            output_config,
            thinking,
        },
    ))
}

/// Groups IR messages into strictly alternating user/assistant turns: consecutive same-role IR
/// messages merge, and turns whose rendered content is empty are not emitted. Well-formed
/// requests open with a user turn, which the merge preserves.
///
/// # Errors
///
/// Returns [`InvalidRequest`] when a tool call's arguments do not encode a JSON object.
fn render_messages(ir: &IrRequest) -> Result<Vec<UpstreamMessage>, InvalidRequest> {
    let mut messages: Vec<UpstreamMessage> = Vec::new();

    for message in &ir.messages {
        let Some((role, blocks)) = render_message(message)? else {
            continue;
        };

        match messages.last_mut() {
            Some(open) if open.role == role => open.content.extend(blocks),

            _ => messages.push(UpstreamMessage {
                role,
                content: blocks,
            }),
        }
    }

    Ok(messages)
}

/// Renders one IR message into its turn side and blocks; [`None`] when the message renders no
/// content at all. IR tool messages fold into user turns carrying `tool_result` blocks, since
/// the API has no tool role.
///
/// Assistant turns hoist reasoning blocks before tool calls — the API requires thinking first —
/// while text keeps its relative order among the remaining blocks.
///
/// # Errors
///
/// Returns [`InvalidRequest`] when a tool call's arguments do not encode a JSON object.
fn render_message(
    message: &IrMessage,
) -> Result<Option<(UpstreamRole, Vec<OutBlock>)>, InvalidRequest> {
    match message.role {
        IrRole::User => {
            let mut blocks = Vec::new();

            for part in &message.parts {
                match part {
                    IrPart::Text { text } => {
                        blocks.push(OutBlock::Typed(TypedBlock::Text { text: text.clone() }))
                    }

                    IrPart::Image { image } => blocks.push(render_image_block(image)),

                    // The IR type system routes only text and images into user content; every
                    // other part kind belongs to a different turn side.
                    IrPart::ToolCall { .. }
                    | IrPart::ToolOutput { .. }
                    | IrPart::Reasoning { .. } => {}
                }
            }

            Ok((!blocks.is_empty()).then_some((UpstreamRole::User, blocks)))
        }

        IrRole::Assistant => {
            let mut reasoning = Vec::new();
            let mut blocks = Vec::new();

            for part in &message.parts {
                match part {
                    IrPart::Reasoning {
                        text,
                        signature,
                        opaque,
                    } => reasoning.push(render_reasoning_block(text, signature, opaque)),

                    IrPart::Text { text } => {
                        blocks.push(OutBlock::Typed(TypedBlock::Text { text: text.clone() }))
                    }

                    IrPart::ToolCall {
                        call_id,
                        name,
                        arguments,
                        ..
                    } => blocks.push(OutBlock::Typed(TypedBlock::ToolUse {
                        id: call_id.clone(),
                        name: name.clone(),
                        input: tool_arguments(arguments)?,
                    })),

                    // Assistant turns carry no images or tool results on this API; the IR type
                    // system routes those into other turn sides.
                    IrPart::Image { .. } | IrPart::ToolOutput { .. } => {}
                }
            }

            // Reasoning blocks that carried no replayable content render nothing; the rest hoist
            // before the turn's text and tool calls.
            let mut content: Vec<OutBlock> = reasoning.into_iter().flatten().collect();

            content.extend(blocks);

            Ok((!content.is_empty()).then_some((UpstreamRole::Assistant, content)))
        }

        IrRole::Tool => {
            let mut blocks = Vec::new();

            for part in &message.parts {
                if let IrPart::ToolOutput {
                    call_id,
                    content,
                    is_error,
                } = part
                {
                    blocks.push(OutBlock::Typed(TypedBlock::ToolResult {
                        tool_use_id: call_id.clone(),
                        content: content.clone(),
                        is_error: (*is_error).then_some(true),
                    }));
                }
            }

            Ok((!blocks.is_empty()).then_some((UpstreamRole::User, blocks)))
        }
    }
}

/// Renders one IR image into an Anthropic image block.
fn render_image_block(image: &IrImage) -> OutBlock {
    match image {
        IrImage::Inline {
            mime_type,
            data_base64,
        } => OutBlock::Typed(TypedBlock::Image {
            source: ImageSourceOut::Base64 {
                media_type: mime_type.clone(),
                data: data_base64.clone(),
            },
        }),

        IrImage::Url(url) => OutBlock::Typed(TypedBlock::Image {
            source: ImageSourceOut::Url { url: url.clone() },
        }),
    }
}

/// Renders one IR reasoning part for an assistant turn: a same-family opaque payload replays
/// verbatim, other-family text synthesizes a thinking block, and content-free cross-family state
/// renders nothing.
///
/// # Panics
///
/// Panics if the synthesized block fails to serialize, which cannot occur for first-party types.
fn render_reasoning_block(
    text: &str,
    signature: &Option<String>,
    opaque: &Value,
) -> Option<OutBlock> {
    if is_anthropic_block(opaque) {
        return Some(OutBlock::Verbatim(opaque.clone()));
    }

    if text.is_empty() {
        return None;
    }

    Some(OutBlock::Typed(TypedBlock::Thinking {
        thinking: text.to_string(),
        signature: signature.clone(),
    }))
}

/// Parses tool call arguments into the JSON object the `input` member requires.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `arguments` when the arguments do not encode a JSON object.
fn tool_arguments(arguments: &str) -> Result<Value, InvalidRequest> {
    if arguments.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }

    let value: Value = serde_json::from_str(arguments).map_err(|error| {
        InvalidRequest::field(
            "arguments",
            format!("Tool call arguments are not valid JSON: {error}"),
        )
    })?;

    if value.is_object() {
        Ok(value)
    } else {
        Err(InvalidRequest::field(
            "arguments",
            "Tool call arguments must encode a JSON object",
        ))
    }
}

/// Probes whether a stored opaque payload is an Anthropic thinking-family block, through a typed
/// probe rather than ad-hoc JSON navigation.
#[derive(Debug, Deserialize)]
struct OpaqueKind {
    /// Block type tag, when the fragment carries one.
    #[serde(default)]
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// Reports whether the opaque fragment is a same-family thinking/redacted block whose payload
/// replays verbatim; every other fragment — cross-family state or bare text — is not.
fn is_anthropic_block(opaque: &Value) -> bool {
    let Ok(probe) = serde_json::from_value::<OpaqueKind>(opaque.clone()) else {
        return false;
    };

    matches!(
        probe.kind.as_deref(),
        Some("thinking") | Some("redacted_thinking")
    )
}

// ---------------------------------------------------------------------------
// Upstream event parsing
// ---------------------------------------------------------------------------

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

/// Renders the canonical reason as an Anthropic `stop_reason` string.
fn finish_reason_to_stop(reason: IrFinishReason) -> &'static str {
    match reason {
        IrFinishReason::Stop => "end_turn",
        IrFinishReason::Length => "max_tokens",
        IrFinishReason::ToolCall => "tool_use",
        IrFinishReason::ContentFilter => "refusal",
    }
}

/// Renders a tool `input` fragment as the JSON-encoded arguments text the IR carries.
fn input_to_arguments(input: &Value) -> String {
    crate::format::render_json(input)
}

// ---------------------------------------------------------------------------
// Inbound request parsing
// ---------------------------------------------------------------------------

/// Strictly parsed `POST /messages` request body: unknown members are 400s, known semantic
/// controls without gateway support are 400s, and presentation-only controls are
/// accepted-and-ignored (README §6.6).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundRequest {
    /// Requested model, routed against the public model list.
    model: String,

    /// Output token budget, required per the spec.
    max_tokens: u64,

    /// Conversation turns.
    messages: Vec<InboundMessage>,

    /// System instructions: a plain string or an array of text blocks.
    #[serde(default)]
    system: Option<StringOrArray<InboundTextBlock>>,

    /// Whether the client wants a streamed response.
    #[serde(default)]
    stream: Option<bool>,

    /// Sampling temperature, passed through verbatim; the upstream decides support.
    #[serde(default)]
    temperature: Option<f64>,

    /// Nucleus sampling threshold, passed through verbatim like `temperature`.
    #[serde(default)]
    top_p: Option<f64>,

    /// Function tools advertised to the model.
    #[serde(default)]
    tools: Option<Vec<InboundTool>>,

    /// Tool selection restriction.
    #[serde(default)]
    tool_choice: Option<InboundToolChoice>,

    /// Legacy thinking control: `disabled` parses into `none`, `enabled`/`adaptive` are
    /// accepted-and-ignored, and `budget_tokens` is a 400 naming the field.
    #[serde(default)]
    thinking: Option<InboundThinking>,

    /// Reasoning level container: `effort` parses into the closed shared vocabulary.
    #[serde(default)]
    output_config: Option<InboundOutputConfig>,

    /// Summary control, accepted and ignored: summaries never enter the IR.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    thinking_summaries: Option<IgnoredAny>,
}

/// One inbound conversation turn, tagged by `role`.
#[derive(Debug, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase", deny_unknown_fields)]
enum InboundMessage {
    /// User turn carrying text, images, and tool results.
    User {
        /// Turn content: a plain string or an array of user blocks.
        content: StringOrArray<InboundUserBlock>,
    },

    /// Assistant turn carrying text, tool calls, and reasoning blocks.
    Assistant {
        /// Turn content: a plain string or an array of assistant blocks.
        content: StringOrArray<InboundAssistantBlock>,
    },
}

/// Content block of a user turn; a member outside the block's own shape is rejected. Placement
/// is enforced by the type itself: `tool_use` and thinking blocks are not user content, so they
/// fail as unknown variants rather than parsing into the wrong turn side.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InboundUserBlock {
    /// Text segment.
    Text {
        /// The text content.
        text: String,
    },

    /// Image segment.
    Image {
        /// Inline data or remote URL source.
        source: InboundImageSource,
    },

    /// Tool result segment.
    ToolResult {
        /// Identifier of the invocation being answered.
        tool_use_id: String,

        /// Result content: a plain string or an array of result parts.
        content: StringOrArray<InboundToolResultPart>,

        /// Whether the tool reported an error.
        #[serde(default)]
        is_error: Option<bool>,
    },
}

/// Content block of an assistant turn; placement is enforced by the type itself like the user
/// blocks. `document` blocks parse only to be rejected as known-unsupported (README §6.6).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InboundAssistantBlock {
    /// Text segment.
    Text {
        /// The text content.
        text: String,
    },

    /// Tool invocation segment.
    ToolUse {
        /// Invocation identifier.
        id: String,

        /// Invoked function name.
        name: String,

        /// JSON arguments object; the fragment's shape is the caller's to define.
        input: Value,
    },

    /// Reasoning segment with its upstream signature.
    Thinking {
        /// Reasoning text.
        thinking: String,

        /// Upstream signature terminating the block.
        #[serde(default)]
        signature: Option<String>,
    },

    /// Opaque redacted reasoning segment.
    RedactedThinking {
        /// Redacted payload, replayed verbatim on same-family upstreams.
        data: Value,
    },

    /// Document segment, known-but-unsupported: the conversion rejects it, and any member
    /// beyond `source` fails even earlier as an unknown field.
    Document {
        /// Document source, ignored: the block is rejected either way.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        source: Option<IgnoredAny>,
    },
}

/// Text block of the `system` array form.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundTextBlock {
    /// Block type tag, always `text` for system content.
    #[serde(rename = "type")]
    kind: String,

    /// The instruction text.
    text: String,
}

/// Image source of an inbound image block.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InboundImageSource {
    /// Base64-encoded inline data.
    Base64 {
        /// MIME type of the image data.
        media_type: String,

        /// Base64-encoded image bytes.
        data: String,
    },

    /// Remote image referenced by URL.
    Url {
        /// Remote URL.
        url: String,
    },
}

/// Content part of a `tool_result` array: text flattens into the tool output, image content is
/// a 400 (the IR tool output has no media channel).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InboundToolResultPart {
    /// Text segment.
    Text {
        /// The text content.
        text: String,
    },

    /// Image segment, known-but-unsupported in tool results.
    Image {
        /// Image source, ignored: the part is rejected either way.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        source: Option<IgnoredAny>,
    },
}

/// Advertised function tool: function tools only — a server-side tool shape fails on its unknown
/// members.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundTool {
    /// Function name the model calls.
    name: String,

    /// Human-readable description.
    #[serde(default)]
    description: Option<String>,

    /// JSON Schema of the accepted arguments.
    input_schema: Value,
}

/// Tool choice restriction: `disable_parallel_tool_use` is accepted and ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundToolChoice {
    /// Choice type: `auto`, `any`, `tool`, or `none`.
    #[serde(rename = "type")]
    kind: String,

    /// Required function name for the `tool` type.
    #[serde(default)]
    name: Option<String>,

    /// Parallel tool-use switch, accepted and ignored: the gateway exposes no such control.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    disable_parallel_tool_use: Option<bool>,
}

/// Legacy thinking control.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundThinking {
    /// Control type: `disabled` parses into `none`, `enabled`/`adaptive` are accepted and
    /// ignored.
    #[serde(rename = "type")]
    kind: String,

    /// Legacy numeric budget mechanism, rejected naming the field like Gemini `thinkingBudget`.
    #[serde(default)]
    budget_tokens: Option<IgnoredAny>,

    /// Display control, accepted and ignored: display never enters the IR.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    display: Option<IgnoredAny>,
}

/// Reasoning level container.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundOutputConfig {
    /// Reasoning effort in the closed shared vocabulary.
    effort: String,

    /// Display control, accepted and ignored like its `thinking` sibling.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    display: Option<IgnoredAny>,
}

/// Parses a `POST /messages` request body into IR.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for malformed JSON and for malformed, unknown, or unsupported
/// members and values.
fn parse_inbound_request(body: &[u8]) -> Result<ParsedRequest, InvalidRequest> {
    let request: InboundRequest = crate::format::parse_client_request(body)?;

    if request.messages.is_empty() {
        return Err(InvalidRequest::field(
            "messages",
            "messages must contain at least one message",
        ));
    }

    let instructions = match &request.system {
        None => None,

        Some(StringOrArray::Text(system)) => Some(system.clone()),

        Some(StringOrArray::Array(blocks)) => {
            for block in blocks {
                if block.kind != "text" {
                    return Err(InvalidRequest::field(
                        "system",
                        "system blocks must be text",
                    ));
                }
            }

            let combined = blocks
                .iter()
                .map(|block| block.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");

            Some(combined)
        }
    };

    let mut messages: Vec<IrMessage> = Vec::new();

    for message in &request.messages {
        match message {
            InboundMessage::User { content } => {
                // A user turn may mix text/images with tool results; the IR separates the sides,
                // so consecutive same-side parts chunk into one IR message each, in order.
                let mut pending: Vec<IrPart> = Vec::new();
                let mut pending_is_tool = false;
                let mut has_pending = false;

                let flush = |messages: &mut Vec<IrMessage>,
                             pending: &mut Vec<IrPart>,
                             pending_is_tool: &mut bool,
                             has_pending: &mut bool| {
                    if *has_pending {
                        messages.push(IrMessage {
                            role: if *pending_is_tool {
                                IrRole::Tool
                            } else {
                                IrRole::User
                            },
                            parts: std::mem::take(pending),
                        });

                        *has_pending = false;
                    }
                };

                let parts = match content {
                    StringOrArray::Text(text) => vec![IrPart::Text { text: text.clone() }],

                    StringOrArray::Array(blocks) => {
                        let mut converted = Vec::with_capacity(blocks.len());

                        for block in blocks {
                            converted.push(user_block_to_part(block)?);
                        }

                        converted
                    }
                };

                for part in parts {
                    let is_tool = matches!(part, IrPart::ToolOutput { .. });

                    if has_pending && is_tool != pending_is_tool {
                        flush(
                            &mut messages,
                            &mut pending,
                            &mut pending_is_tool,
                            &mut has_pending,
                        );
                    }

                    pending_is_tool = is_tool;
                    has_pending = true;
                    pending.push(part);
                }

                flush(
                    &mut messages,
                    &mut pending,
                    &mut pending_is_tool,
                    &mut has_pending,
                );
            }

            InboundMessage::Assistant { content } => {
                let parts = match content {
                    StringOrArray::Text(text) => vec![IrPart::Text { text: text.clone() }],

                    StringOrArray::Array(blocks) => {
                        let mut converted = Vec::with_capacity(blocks.len());

                        for block in blocks {
                            converted.push(assistant_block_to_part(block)?);
                        }

                        converted
                    }
                };

                messages.push(IrMessage {
                    role: IrRole::Assistant,
                    parts,
                });
            }
        }
    }

    let mut tools = Vec::new();

    if let Some(tools_in) = &request.tools {
        for tool in tools_in {
            if !tool.input_schema.is_object() {
                return Err(InvalidRequest::field(
                    "input_schema",
                    "Tool input_schema must be a JSON object",
                ));
            }

            tools.push(IrTool {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: Some(tool.input_schema.clone()),
            });
        }
    }

    let tool_choice = match &request.tool_choice {
        None => None,

        Some(choice) => {
            crate::format::reject_tool_choice_without_tools(tools.is_empty())?;

            Some(match choice.kind.as_str() {
                "auto" => IrToolChoice::Auto,
                "none" => IrToolChoice::None,
                "any" => IrToolChoice::Required,

                "tool" => match &choice.name {
                    Some(name) => IrToolChoice::Function { name: name.clone() },

                    None => {
                        return Err(InvalidRequest::field(
                            "tool_choice",
                            "tool_choice `tool` requires a `name`",
                        ));
                    }
                },

                other => {
                    return Err(InvalidRequest::field(
                        "tool_choice",
                        format!("Unknown tool_choice `{other}`"),
                    ));
                }
            })
        }
    };

    let thinking_effort = match &request.thinking {
        None => None,

        Some(thinking) => {
            if thinking.budget_tokens.is_some() {
                return Err(InvalidRequest::field(
                    "budget_tokens",
                    "budget_tokens is not supported; use output_config.effort",
                ));
            }

            match thinking.kind.as_str() {
                "disabled" => Some(IrReasoningLevel::None),

                // Accepted and ignored: no replayable state, no IR slot.
                "enabled" | "adaptive" => None,

                other => {
                    return Err(InvalidRequest::field(
                        "thinking",
                        format!("Unknown thinking type `{other}`"),
                    ));
                }
            }
        }
    };

    let output_effort = request
        .output_config
        .as_ref()
        .map(|output_config| IrReasoningLevel::parse(&output_config.effort, "output_config"))
        .transpose()?;

    // A disabled legacy control contradicts an explicit effort level; an ignored legacy control
    // beside one is simply redundant.
    if thinking_effort.is_some() && output_effort.is_some() {
        return Err(InvalidRequest::field(
            "thinking",
            "thinking and output_config are mutually exclusive",
        ));
    }

    Ok(ParsedRequest {
        ir: IrRequest {
            model: request.model,
            instructions,
            messages,
            tools,
            tool_choice,
            temperature: request.temperature,
            top_p: request.top_p,
            max_output_tokens: Some(request.max_tokens),
            response_format: None,
            reasoning_effort: output_effort.or(thinking_effort),
        },
        is_streaming: request.stream.unwrap_or(false),
        // Usage rides every Anthropic response unconditionally; no opt-in chunk exists.
        should_include_usage: false,
    })
}

/// Converts one user content block into its IR part.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for image content inside tool results.
fn user_block_to_part(block: &InboundUserBlock) -> Result<IrPart, InvalidRequest> {
    match block {
        InboundUserBlock::Text { text } => Ok(IrPart::Text { text: text.clone() }),

        InboundUserBlock::Image { source } => Ok(IrPart::Image {
            image: match source {
                InboundImageSource::Base64 { media_type, data } => IrImage::Inline {
                    mime_type: media_type.clone(),
                    data_base64: data.clone(),
                },

                InboundImageSource::Url { url } => IrImage::Url(url.clone()),
            },
        }),

        InboundUserBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let text = match content {
                StringOrArray::Text(text) => text.clone(),

                StringOrArray::Array(parts) => {
                    let mut combined = String::new();

                    for part in parts {
                        match part {
                            InboundToolResultPart::Text { text } => {
                                if !combined.is_empty() {
                                    combined.push_str("\n\n");
                                }

                                combined.push_str(text);
                            }

                            InboundToolResultPart::Image { .. } => {
                                return Err(InvalidRequest::field(
                                    "content",
                                    "Image content inside tool results is not supported",
                                ));
                            }
                        }
                    }

                    combined
                }
            };

            Ok(IrPart::ToolOutput {
                call_id: tool_use_id.clone(),
                content: text,
                is_error: is_error.unwrap_or(false),
            })
        }
    }
}

/// Converts one assistant content block into its IR part: thinking blocks keep their verbatim
/// replay payload so same-family upstreams receive them byte-exact.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for `document` blocks and for tool arguments that do not encode a
/// JSON object.
fn assistant_block_to_part(block: &InboundAssistantBlock) -> Result<IrPart, InvalidRequest> {
    match block {
        InboundAssistantBlock::Text { text } => Ok(IrPart::Text { text: text.clone() }),

        InboundAssistantBlock::ToolUse { id, name, input } => {
            if !input.is_object() {
                return Err(InvalidRequest::field(
                    "input",
                    "Tool use input must be a JSON object",
                ));
            }

            let arguments = crate::format::render_json(input);

            crate::format::arguments_as_object(&arguments)?;

            Ok(IrPart::ToolCall {
                call_id: id.clone(),
                name: name.clone(),
                arguments,
                thought_signature: None,
            })
        }

        InboundAssistantBlock::Thinking {
            thinking,
            signature,
        } => {
            let block = TypedBlock::Thinking {
                thinking: thinking.clone(),
                signature: signature.clone(),
            };

            Ok(IrPart::Reasoning {
                text: thinking.clone(),
                signature: signature.clone(),
                opaque: block_opaque(&block),
            })
        }

        InboundAssistantBlock::RedactedThinking { data } => {
            let block = TypedBlock::RedactedThinking { data: data.clone() };

            Ok(IrPart::Reasoning {
                text: String::new(),
                signature: None,
                opaque: block_opaque(&block),
            })
        }

        InboundAssistantBlock::Document { .. } => Err(InvalidRequest::field(
            "content",
            "document blocks are not supported",
        )),
    }
}

// ---------------------------------------------------------------------------
// Client rendering
// ---------------------------------------------------------------------------

/// Marker of the inbound Anthropic Messages client format.
#[derive(Debug)]
pub(crate) struct Format;

impl ClientFormat for Format {
    type JsonResponse = MessagesResponse;

    const ENDPOINT_TYPE: ApiLogEndpointType = ApiLogEndpointType::AnthropicMessages;

    fn parse_request(body: &[u8], _route: &RouteContext) -> Result<ParsedRequest, InvalidRequest> {
        parse_inbound_request(body)
    }

    fn sse_renderer(
        _should_include_usage: bool,
        public_id: String,
    ) -> Box<dyn ClientSseRenderer + Send> {
        Box::new(SseRenderer::new(public_id))
    }

    fn render_json_response(public_id: &str, aggregation: &IrAggregation) -> Self::JsonResponse {
        render_json_response(public_id, aggregation)
    }
}

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

/// Token accounting in the native Messages shape: totals always, cache columns when reported,
/// and no reasoning breakdown.
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
}

impl ResponseUsage {
    /// Renders one observation, carrying reported cache columns and omitting the rest.
    fn render(usage: IrUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_input_tokens: usage.cache_write_tokens,
            cache_read_input_tokens: usage.cached_tokens,
        }
    }

    /// Zero usage, opening the streamed message before any report arrived.
    fn zero() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }
    }
}

/// Renders an aggregated completion as a non-streaming Messages response.
fn render_json_response(model: &str, aggregation: &IrAggregation) -> MessagesResponse {
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
            input: tool_arguments_to_input(&tool_call.arguments),
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

/// Renders tool arguments into the `input` object: the parsed object when the arguments encode
/// one, the empty object for the degraded remainder, which carries no inventable shape.
fn tool_arguments_to_input(arguments: &str) -> Value {
    if arguments.trim().is_empty() {
        return Value::Object(Default::default());
    }

    match serde_json::from_str::<Value>(arguments) {
        Ok(value) if value.is_object() => value,
        _ => Value::Object(Default::default()),
    }
}

/// Streaming client response renderer producing Messages SSE frames.
#[derive(Debug)]
struct SseRenderer {
    /// Gateway-issued message identifier.
    id: String,

    /// Public model name echoed to the client.
    model: String,

    /// Next stream content index to assign.
    next_index: usize,

    /// Currently open content block, closed lazily when the next block starts or the turn ends.
    open_block: Option<OpenOutBlock>,

    /// Whether the `message_start` frame was emitted; the gateway sends exactly one
    /// [`IrEvent::MessageStart`], and a repeated one renders nothing.
    is_started: bool,

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

    /// Zero usage; the terminal delta carries the observed report.
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
    error: error_anthropic::AnthropicErrorBody,
}

impl SseRenderer {
    /// Creates a renderer for one streamed completion.
    fn new(model: String) -> Self {
        Self {
            id: format!("msg_{}", ids::new_uuid()),
            model,
            next_index: 0,
            open_block: None,
            is_started: false,
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

    /// Emits the terminal `message_delta` frame carrying the stop reason and the latest usage.
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
        match event {
            IrEvent::MessageStart { .. } => {
                if self.is_started {
                    return Vec::new();
                }

                self.is_started = true;

                vec![Self::frame(
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
                            usage: ResponseUsage::zero(),
                        },
                    },
                )]
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

            IrEvent::Finish(reason) => {
                self.finish = Some(*reason);

                let mut frames = self.close_open_block();

                frames.push(self.message_delta_frame(*reason));

                frames
            }

            IrEvent::Usage(usage) => {
                self.usage = Some(*usage);

                Vec::new()
            }

            IrEvent::Done => {
                let mut frames = self.close_open_block();

                // A clean end-of-stream without a finish degrades to `end_turn`, so streamed
                // and aggregated renderings of the same stream agree; a reported finish
                // already emitted its delta.
                if self.finish.is_none() {
                    frames.push(self.message_delta_frame(IrFinishReason::Stop));
                }

                frames.push(Self::frame(
                    "message_stop",
                    &MessageStopPayload {
                        kind: "message_stop",
                    },
                ));

                frames
            }
        }
    }

    /// Renders a mid-stream error as a terminal `error` event, classified once through the
    /// shared API error (REFACTOR §3).
    fn error_frame(&mut self, error: &crate::api::Error) -> String {
        Self::frame(
            "error",
            &StreamErrorPayload {
                kind: "error",
                error: error_anthropic::render_body(error),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IrPart, IrReasoningBlock, IrTool};
    use serde_json::{Value, json};

    /// Serializes one upstream request body for inspection.
    ///
    /// # Panics
    ///
    /// Panics if the rendered body fails to serialize.
    fn upstream_body(ir: &IrRequest, is_streaming: bool) -> Value {
        #[expect(
            clippy::expect_used,
            reason = "fixture requests render; a failing render is the failure"
        )]
        let (_, body) = render_request(ir, "https://api.example", is_streaming)
            .expect("the fixture request renders");

        #[expect(
            clippy::expect_used,
            reason = "the rendered body is under test; a failing render is the failure"
        )]
        serde_json::to_value(&body).expect("the rendered body serializes")
    }

    /// Reads the JSON value at a pointer path of a rendered body.
    ///
    /// # Panics
    ///
    /// Panics if the path is absent: a missing path is the failure under test.
    fn at<'a>(value: &'a Value, pointer: &str) -> &'a Value {
        #[expect(
            clippy::expect_used,
            reason = "rendered bodies are under test; a missing path is the failure"
        )]
        value
            .pointer(pointer)
            .expect("the rendered body carries the expected path")
    }

    /// Reads one parsed message part by position.
    ///
    /// # Panics
    ///
    /// Panics if the position is absent: a missing part is the failure under test.
    fn part(parts: &[IrPart], index: usize) -> &IrPart {
        #[expect(
            clippy::expect_used,
            reason = "parsed messages are under test; a missing part is the failure"
        )]
        parts
            .get(index)
            .expect("the parsed message carries the expected part")
    }

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

    /// Sets one member of a JSON object fixture body.
    ///
    /// # Panics
    ///
    /// Panics if the body is not an object, which cannot occur for the fixtures.
    fn set_member(body: &mut Value, key: &str, value: Value) {
        #[expect(
            clippy::expect_used,
            reason = "fixture bodies are objects by construction"
        )]
        body.as_object_mut()
            .expect("fixture bodies are JSON objects")
            .insert(key.to_string(), value);
    }

    /// Builds the IR of one alternating conversation with a tool round-trip.
    fn conversation_ir() -> IrRequest {
        IrRequest {
            model: "claude-test".to_string(),
            instructions: Some("be terse".to_string()),
            messages: vec![
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: "hello".to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: "again".to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![
                        IrPart::Reasoning {
                            text: "draft".to_string(),
                            signature: Some("sig".to_string()),
                            opaque: Value::Null,
                        },
                        IrPart::Text {
                            text: "calling".to_string(),
                        },
                        IrPart::ToolCall {
                            call_id: "call_1".to_string(),
                            name: "get_weather".to_string(),
                            arguments: r#"{"city":"Paris"}"#.to_string(),
                            thought_signature: None,
                        },
                    ],
                },
                IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id: "call_1".to_string(),
                        content: "sunny".to_string(),
                        is_error: true,
                    }],
                },
            ],
            max_output_tokens: Some(256),
            ..IrRequest::default()
        }
    }

    /// Groups IR messages into strictly alternating turns: consecutive same-role messages merge,
    /// the tool message folds into a user turn carrying its `is_error` result, reasoning
    /// synthesizes a thinking block ahead of the tool call, and instructions render as the
    /// system string.
    ///
    /// # Panics
    ///
    /// Panics if the rendered turns diverge from the documented grouping.
    #[test]
    fn groups_turns_merges_roles_and_carries_tool_errors() {
        let body = upstream_body(&conversation_ir(), false);

        assert_eq!(at(&body, "/model"), &json!("claude-test"));
        assert_eq!(at(&body, "/max_tokens"), &json!(256));
        assert_eq!(at(&body, "/system"), &json!("be terse"));
        assert_eq!(at(&body, "/stream"), &json!(false));
        assert_eq!(at(&body, "/messages/0/role"), &json!("user"));
        assert_eq!(at(&body, "/messages/1/role"), &json!("assistant"));
        assert_eq!(at(&body, "/messages/2/role"), &json!("user"));

        // Consecutive user messages merged into one turn with both texts.
        assert_eq!(
            at(&body, "/messages/0/content"),
            &json!([
                {"type": "text", "text": "hello"},
                {"type": "text", "text": "again"},
            ]),
        );

        // Reasoning hoists before the tool call; the cross-family text synthesizes a thinking
        // block carrying its signature.
        assert_eq!(
            at(&body, "/messages/1/content"),
            &json!([
                {"type": "thinking", "thinking": "draft", "signature": "sig"},
                {"type": "text", "text": "calling"},
                {
                    "type": "tool_use",
                    "id": "call_1",
                    "name": "get_weather",
                    "input": {"city": "Paris"},
                },
            ]),
        );

        // The tool message folds into a user turn; the error flag rides along.
        assert_eq!(
            at(&body, "/messages/2/content"),
            &json!([{
                "type": "tool_result",
                "tool_use_id": "call_1",
                "content": "sunny",
                "is_error": true,
            }]),
        );
    }

    /// Renders `max_tokens` from the client's budget, else the documented synthesis constant;
    /// empty assistant content never emits a turn.
    ///
    /// # Panics
    ///
    /// Panics if the rendered budget or turn filtering diverges from the contract.
    #[test]
    fn synthesizes_max_tokens_and_skips_empty_turns() {
        let ir = IrRequest {
            model: "m".to_string(),
            messages: vec![IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: "hi".to_string(),
                }],
            }],
            ..IrRequest::default()
        };

        assert_eq!(
            at(&upstream_body(&ir, true), "/max_tokens"),
            &json!(SYNTHESIZED_MAX_TOKENS)
        );

        let with_budget = IrRequest {
            max_output_tokens: Some(64),
            ..ir.clone()
        };

        assert_eq!(
            at(&upstream_body(&with_budget, true), "/max_tokens"),
            &json!(64)
        );

        let empty_assistant = IrRequest {
            messages: vec![
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: "hi".to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::Assistant,
                    parts: Vec::new(),
                },
            ],
            ..ir
        };

        let body = upstream_body(&empty_assistant, false);

        #[expect(
            clippy::expect_used,
            reason = "the rendered turns are under test; a missing turn is the failure"
        )]
        let messages = body
            .get("messages")
            .expect("messages render as an array")
            .as_array()
            .expect("messages render as an array");

        assert_eq!(messages.len(), 1);
    }

    /// Renders the reasoning level verbatim into its own slot: `none` becomes the native
    /// `thinking: disabled` spelling, every other level becomes `output_config.effort`, and an
    /// absent effort renders neither (README §6.4).
    ///
    /// # Panics
    ///
    /// Panics if a rendered slot diverges from the contract.
    #[test]
    fn renders_the_effort_level_verbatim_or_disabled() {
        let base = IrRequest {
            model: "m".to_string(),
            messages: vec![IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: "hi".to_string(),
                }],
            }],
            ..IrRequest::default()
        };

        for (level, effort) in [
            (IrReasoningLevel::Minimal, "minimal"),
            (IrReasoningLevel::Low, "low"),
            (IrReasoningLevel::Medium, "medium"),
            (IrReasoningLevel::High, "high"),
            (IrReasoningLevel::XHigh, "xhigh"),
            (IrReasoningLevel::Max, "max"),
        ] {
            let ir = IrRequest {
                reasoning_effort: Some(level),
                ..base.clone()
            };

            let body = upstream_body(&ir, false);

            assert_eq!(at(&body, "/output_config"), &json!({"effort": effort}));
            assert!(body.get("thinking").is_none());
        }

        let ir = IrRequest {
            reasoning_effort: Some(IrReasoningLevel::None),
            ..base.clone()
        };

        let body = upstream_body(&ir, false);

        assert_eq!(at(&body, "/thinking"), &json!({"type": "disabled"}));
        assert!(body.get("output_config").is_none());

        let body = upstream_body(&base, false);

        assert!(body.get("output_config").is_none());
        assert!(body.get("thinking").is_none());
    }

    /// Replays a same-family opaque thinking block verbatim and maps the four tool-choice modes;
    /// a stricter-than-text response format fails explicitly naming the field.
    ///
    /// # Panics
    ///
    /// Panics if the replay, the mappings, or the rejection diverge from the contract.
    #[test]
    fn replays_opaque_blocks_maps_tool_choice_and_rejects_response_format() {
        let opaque: Value = json!({
            "type": "thinking",
            "thinking": "upstream text",
            "signature": "upstream-sig",
        });

        let ir = IrRequest {
            model: "m".to_string(),
            messages: vec![IrMessage {
                role: IrRole::Assistant,
                parts: vec![IrPart::Reasoning {
                    text: "rewritten?".to_string(),
                    signature: None,
                    opaque: opaque.clone(),
                }],
            }],
            tool_choice: Some(IrToolChoice::Function {
                name: "search".to_string(),
            }),
            ..IrRequest::default()
        };

        let body = upstream_body(&ir, false);

        assert_eq!(at(&body, "/messages/0/content"), &json!([opaque]));
        assert_eq!(
            at(&body, "/tool_choice"),
            &json!({"type": "tool", "name": "search"}),
        );

        for (choice, rendered) in [
            (IrToolChoice::Auto, json!({"type": "auto"})),
            (IrToolChoice::None, json!({"type": "none"})),
            (IrToolChoice::Required, json!({"type": "any"})),
        ] {
            let ir = IrRequest {
                tool_choice: Some(choice),
                ..IrRequest::default()
            };

            // The choice mapping needs no messages; only the choice is inspected.
            #[expect(
                clippy::expect_used,
                reason = "the tool choice is constructed to render"
            )]
            let (_, rendered_body) =
                render_request(&ir, "https://api.example", false).expect("the tool choice renders");

            #[expect(
                clippy::expect_used,
                reason = "the rendered body is under test; a failing render is the failure"
            )]
            let value = serde_json::to_value(&rendered_body).expect("the body serializes");

            assert_eq!(at(&value, "/tool_choice"), &rendered);
        }

        let ir = IrRequest {
            response_format: Some(IrResponseFormat::JsonSchema {
                name: "shape".to_string(),
                schema: json!({}),
            }),
            ..IrRequest::default()
        };

        #[expect(
            clippy::expect_used,
            reason = "the response format is constructed to be rejected"
        )]
        let error = render_request(&ir, "https://api.example", false)
            .expect_err("a JSON response format has no Anthropic slot");

        assert_eq!(error.param.as_deref(), Some("response_format"));
    }

    /// Renders a tool call whose arguments text is empty as an empty `input` object: the member
    /// carries the call's own payload, never the tool schema.
    ///
    /// # Panics
    ///
    /// Panics if the rendered `input` diverges from the empty object.
    #[test]
    fn renders_empty_tool_arguments_as_an_empty_input_object() {
        let ir = IrRequest {
            model: "m".to_string(),
            max_output_tokens: Some(16),
            messages: vec![IrMessage {
                role: IrRole::Assistant,
                parts: vec![IrPart::ToolCall {
                    call_id: "call_1".to_string(),
                    name: "ping".to_string(),
                    arguments: String::new(),
                    thought_signature: None,
                }],
            }],
            ..IrRequest::default()
        };

        let body = upstream_body(&ir, false);

        assert_eq!(at(&body, "/messages/0/content/0/input"), &json!({}));
    }

    /// Parses one inbound body covering every content shape: system string, text and both image
    /// sources, tool calls with object inputs, error tool results, and thinking blocks whose
    /// opaque payload replays byte-exact upstream.
    ///
    /// # Panics
    ///
    /// Panics if the parsed IR diverges from the documented mapping.
    #[test]
    fn parses_every_inbound_content_shape() {
        let body = json!({
            "model": "claude-test",
            "max_tokens": 128,
            "stream": true,
            "system": "be terse",
            "temperature": 0.7,
            "top_p": 0.9,
            "output_config": {"effort": "high"},
            "tools": [{
                "name": "get_weather",
                "description": "City weather",
                "input_schema": {"type": "object"},
            }],
            "tool_choice": {"type": "tool", "name": "get_weather"},
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "sky?"},
                    {"type": "image", "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": "aGVsbG8=",
                    }},
                    {"type": "image", "source": {
                        "type": "url",
                        "url": "https://example.com/i.png",
                    }},
                ]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "check", "signature": "sig"},
                    {"type": "redacted_thinking", "data": "enc"},
                    {"type": "text", "text": "looking"},
                    {"type": "tool_use", "id": "call_1", "name": "get_weather",
                     "input": {"city": "Paris"}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1",
                     "content": "sunny", "is_error": true},
                ]},
            ],
        });

        #[expect(
            clippy::expect_used,
            reason = "the fixture body is constructed to parse"
        )]
        let parsed =
            parse_inbound_request(body.to_string().as_bytes()).expect("every content shape parses");

        assert!(parsed.is_streaming);
        assert_eq!(parsed.ir.model, "claude-test");
        assert_eq!(parsed.ir.max_output_tokens, Some(128));
        assert_eq!(parsed.ir.instructions.as_deref(), Some("be terse"));
        assert_eq!(parsed.ir.temperature, Some(0.7));
        assert_eq!(parsed.ir.top_p, Some(0.9));
        assert_eq!(parsed.ir.reasoning_effort, Some(IrReasoningLevel::High),);
        assert_eq!(
            parsed.ir.tools,
            vec![IrTool {
                name: "get_weather".to_string(),
                description: Some("City weather".to_string()),
                parameters: Some(json!({"type": "object"})),
            }],
        );
        assert_eq!(
            parsed.ir.tool_choice,
            Some(IrToolChoice::Function {
                name: "get_weather".to_string(),
            }),
        );

        assert_eq!(parsed.ir.messages.len(), 3);

        #[expect(clippy::expect_used, reason = "the fixture carries three messages")]
        let user = parsed.ir.messages.first().expect("the user message parses");

        assert_eq!(user.role, IrRole::User);
        assert_eq!(user.parts.len(), 3);
        assert!(matches!(
            part(&user.parts, 1),
            IrPart::Image {
                image: IrImage::Inline { .. },
            },
        ));
        assert_eq!(
            part(&user.parts, 2),
            &IrPart::Image {
                image: IrImage::Url("https://example.com/i.png".to_string()),
            },
        );

        #[expect(
            clippy::expect_used,
            reason = "the fixture carries an assistant message"
        )]
        let assistant = parsed
            .ir
            .messages
            .get(1)
            .expect("the assistant message parses");

        assert_eq!(assistant.parts.len(), 4);
        assert_eq!(
            part(&assistant.parts, 2),
            &IrPart::Text {
                text: "looking".to_string(),
            },
        );
        assert_eq!(
            part(&assistant.parts, 3),
            &IrPart::ToolCall {
                call_id: "call_1".to_string(),
                name: "get_weather".to_string(),
                arguments: r#"{"city":"Paris"}"#.to_string(),
                thought_signature: None,
            },
        );

        // The thinking block keeps its verbatim replay payload: the opaque value equals the
        // inbound block exactly.
        assert_eq!(
            part(&assistant.parts, 0),
            &IrPart::Reasoning {
                text: "check".to_string(),
                signature: Some("sig".to_string()),
                opaque: json!({"type": "thinking", "thinking": "check",
                                "signature": "sig"}),
            },
        );

        #[expect(clippy::expect_used, reason = "the fixture carries a tool message")]
        let tool = parsed.ir.messages.get(2).expect("the tool message parses");

        assert_eq!(tool.role, IrRole::Tool);
        assert_eq!(
            tool.parts,
            vec![IrPart::ToolOutput {
                call_id: "call_1".to_string(),
                content: "sunny".to_string(),
                is_error: true,
            }],
        );
    }

    /// Rejects unsupported and malformed inbound controls exactly where the contract pins them:
    /// `stop_sequences`, `budget_tokens` (naming the field), `document` blocks, images inside
    /// tool results, a missing `max_tokens`, and unknown members — while `thinking: disabled`
    /// parses into `none` and `enabled`/`adaptive` plus `display` controls are accepted and
    /// ignored.
    ///
    /// # Panics
    ///
    /// Panics if any rejection lands outside its documented wording.
    #[test]
    fn rejects_unsupported_controls_and_honors_thinking_dispositions() {
        let base = || {
            json!({
                "model": "m",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "hi"}],
            })
        };

        let cases: Vec<(Value, Option<&str>)> = vec![
            (
                json!({"stop_sequences": ["x"], "model": "m", "max_tokens": 16,
                       "messages": [{"role": "user", "content": "hi"}]}),
                None,
            ),
            (
                json!({"model": "m", "max_tokens": 16,
                       "thinking": {"type": "enabled", "budget_tokens": 1000},
                       "messages": [{"role": "user", "content": "hi"}]}),
                Some("budget_tokens"),
            ),
            (
                json!({"model": "m", "max_tokens": 16, "messages": [
                    {"role": "assistant", "content": [
                        {"type": "document", "source": {"type": "text", "data": "x"}},
                    ]},
                ]}),
                Some("content"),
            ),
            (
                json!({"model": "m", "max_tokens": 16, "messages": [
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "c",
                         "content": [{"type": "image",
                                      "source": {"type": "url",
                                                 "url": "https://example.com/i.png"}}]},
                    ]},
                ]}),
                Some("content"),
            ),
            (
                json!({"model": "m",
                       "messages": [{"role": "user", "content": "hi"}]}),
                None,
            ),
            (
                json!({"model": "m", "max_tokens": 16, "bogus": true,
                       "messages": [{"role": "user", "content": "hi"}]}),
                None,
            ),
        ];

        for (mut body, param) in cases {
            // Every case starts from the valid base shape unless it already states its own.
            if body.get("messages").is_none() {
                body = base();
            }

            #[expect(
                clippy::expect_used,
                reason = "each control is constructed to be rejected"
            )]
            let error = parse_inbound_request(body.to_string().as_bytes())
                .expect_err("the unsupported control must be rejected");

            assert_eq!(error.param.as_deref(), param, "wrong param for {error}");
        }

        // `thinking: disabled` parses into `none`; `enabled`/`adaptive` and both `display`
        // controls parse without entering the IR.
        for (thinking, output_config, expected) in [
            (
                Some(json!({"type": "disabled"})),
                None,
                Some(IrReasoningLevel::None),
            ),
            (Some(json!({"type": "enabled"})), None, None),
            (Some(json!({"type": "adaptive"})), None, None),
            (
                Some(json!({"type": "enabled", "display": "summary"})),
                None,
                None,
            ),
            (
                None,
                Some(json!({"effort": "low", "display": "none"})),
                Some(IrReasoningLevel::Low),
            ),
        ] {
            let mut body = base();

            if let Some(thinking) = thinking {
                set_member(&mut body, "thinking", thinking);
            }

            if let Some(output_config) = output_config {
                set_member(&mut body, "output_config", output_config);
            }

            set_member(&mut body, "thinking_summaries", json!(["summary"]));

            #[expect(
                clippy::expect_used,
                reason = "each thinking disposition is constructed to parse"
            )]
            let parsed = parse_inbound_request(body.to_string().as_bytes())
                .expect("the thinking disposition parses");

            assert_eq!(parsed.ir.reasoning_effort, expected);
        }

        // An unknown effort string is a 400 naming `output_config`.
        let mut body = base();

        set_member(&mut body, "output_config", json!({"effort": "ultra"}));

        #[expect(
            clippy::expect_used,
            reason = "the unknown effort is constructed to be rejected"
        )]
        let error = parse_inbound_request(body.to_string().as_bytes())
            .expect_err("an unknown effort must be rejected");

        assert_eq!(error.param.as_deref(), Some("output_config"));
    }

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

        // Cache columns ride through when reported; the reasoning breakdown stays null because
        // the Messages usage shape has no such column.
        assert_eq!(
            at(&body, "/usage"),
            &json!({
                "input_tokens": 4,
                "output_tokens": 8,
                "cache_read_input_tokens": 1,
            }),
        );
    }

    /// Streams one turn through the client renderer: block indexes ascend across text,
    /// thinking, and tool blocks; the signature closes the thinking block; the tool call
    /// emits whole on its done event; and the terminal delta carries the stop reason with the
    /// last usage before `message_stop`.
    ///
    /// # Panics
    ///
    /// Panics if the rendered frame sequence diverges from the native stream shape.
    #[test]
    fn streams_native_frames_with_ascending_block_indexes() {
        let mut renderer = SseRenderer::new("claude-test".to_string());

        let kinds = |frames: &[String]| {
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
                .collect::<Vec<_>>()
        };

        let payloads = |frames: &[String]| {
            frames
                .iter()
                .map(|frame| {
                    #[expect(
                        clippy::expect_used,
                        reason = "the rendered frames are under test; a malformed frame is the \
                                  failure"
                    )]
                    serde_json::from_str::<Value>(
                        frame
                            .split_once("data: ")
                            .expect("every frame carries a data line")
                            .1,
                    )
                    .expect("frame data is rendered JSON")
                })
                .collect::<Vec<_>>()
        };

        let start = renderer.render(&IrEvent::MessageStart {
            id: "up".to_string(),
            model: "up-model".to_string(),
        });

        assert_eq!(kinds(&start), ["message_start"]);
        assert_eq!(
            at(first_payload(&payloads(&start)), "/message/model"),
            &json!("claude-test")
        );

        let text = renderer.render(&IrEvent::TextDelta("hi".to_string()));

        assert_eq!(kinds(&text), ["content_block_start", "content_block_delta"],);

        let text_payloads = payloads(&text);

        assert_eq!(at(payload_at(&text_payloads, 0), "/index"), &json!(0));
        assert_eq!(
            at(payload_at(&text_payloads, 1), "/delta"),
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

        renderer.render(&IrEvent::Usage(IrUsage {
            input_tokens: 3,
            output_tokens: 6,
            cached_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
        }));

        let finish = renderer.render(&IrEvent::Finish(IrFinishReason::Stop));

        assert_eq!(kinds(&finish), ["message_delta"]);

        let finish_payloads = payloads(&finish);

        assert_eq!(
            at(payload_at(&finish_payloads, 0), "/delta"),
            &json!({"stop_reason": "end_turn", "stop_sequence": Value::Null}),
        );
        assert_eq!(
            at(payload_at(&finish_payloads, 0), "/usage"),
            &json!({"input_tokens": 3, "output_tokens": 6}),
        );

        let terminal = renderer.render(&IrEvent::Done);

        assert_eq!(kinds(&terminal), ["message_stop"]);
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
}
