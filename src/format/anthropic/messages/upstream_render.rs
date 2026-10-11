//! Upstream-request rendering for the Anthropic Messages codec: renders an IR request into the
//! `anthropic_messages` upstream payload.
//!
//! Upstream requests render `POST {base}/v1/messages` (`x-api-key`, pinned `anthropic-version`
//! headers are attached by the gateway): the `system` string from IR instructions, IR messages
//! grouped into strictly alternating user/assistant turns, assistant reasoning blocks before their
//! tool calls, `tool_result` blocks carrying `is_error`, typed function tools, the four-mode
//! `tool_choice`, verbatim sampling parameters, `max_tokens` from the client's budget else the
//! synthesis constant, and `output_config` — the effort verbatim plus, for a JSON-schema response
//! format, the official `JSONOutputFormatParam` beside it — or `thinking: disabled` for `none`
//! (`docs/reference/inference.md`).

use super::{ImageSourceOut, OutBlock, TypedBlock, is_anthropic_block};
use crate::{
    format::RoleRun,
    ir::{
        InvalidRequest, IrImage, IrMessage, IrPart, IrRequest, IrResponseFormat, IrRole, IrTool,
        IrToolChoice, StateFamily, StateItem,
    },
    vocabulary::ReasoningLevel,
};
use serde::Serialize;
use serde_json::Value;

#[cfg(test)]
mod tests;

/// Fixed synthesis constant rendered as `max_tokens` when the client sent no
/// `max_output_tokens`: the API requires the field, so the gateway documents this stand-in
/// rather than inventing a caller-specific budget (`docs/design/providers.md`).
const SYNTHESIZED_MAX_TOKENS: u64 = 16384;

/// The upstream Anthropic Messages request payload.
///
/// The base render — [`render_request_body`] — never emits a compaction member: it is the
/// strip-compaction-controls form the compaction executor uses for continuation calls, so an
/// internal request can never carry the caller's compaction controls back upstream. The native
/// compaction dispatches ([`render_on_demand_body`], [`render_threshold_body`]) add their member
/// explicitly.
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

    /// The effort and output-format container, present when the request carries either member;
    /// omitted when it carries neither (the `none` effort renders `thinking: disabled` instead).
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<OutputConfig<'a>>,

    /// Native off spelling, rendered only for the `none` effort level
    /// (`docs/reference/inference.md`).
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingDisabled>,

    /// The native on-demand compaction control, rendered only by
    /// [`render_on_demand_body`]; the base render never emits it.
    #[serde(skip_serializing_if = "Option::is_none")]
    compaction: Option<OnDemandControl<'a>>,

    /// The native threshold compaction edits, rendered only by [`render_threshold_body`]; the
    /// base render never emits it.
    #[serde(skip_serializing_if = "Option::is_none")]
    context_management: Option<ContextManagementBody<'a>>,
}

/// The native on-demand compaction control — the upstream body's `compaction` member.
#[derive(Debug, Serialize)]
struct OnDemandControl<'a> {
    /// Always `summarize`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// Custom summarization instructions, omitted when the caller sent none.
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
}

/// The native threshold compaction edits — the upstream body's `context_management` member,
/// carrying the caller's vendor-shaped edit objects verbatim.
#[derive(Debug, Serialize)]
struct ContextManagementBody<'a> {
    /// The vendor-shaped edit objects.
    edits: &'a [crate::ir::compaction::ThresholdEdit],
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
    input_schema: InputSchema<'a>,
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

/// The upstream `output_config` container — the official `OutputConfigParam`: the reasoning
/// effort and the JSON-schema output format beside it, each present when the request carries one.
#[derive(Debug, Serialize)]
struct OutputConfig<'a> {
    /// Reasoning effort in the closed shared vocabulary, written verbatim; omitted when the
    /// client sent no effort.
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<&'a str>,

    /// The JSON-schema output format, present only for a schema-constrained response.
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<JsonOutputFormat<'a>>,
}

/// The upstream `output_config.format` member — the official `JSONOutputFormatParam`, whose
/// `type` and `schema` members are both required.
#[derive(Debug, Serialize)]
struct JsonOutputFormat<'a> {
    /// Always `json_schema`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// The client's JSON Schema of the response.
    schema: &'a Value,
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
#[serde(tag = "type", rename = "object", deny_unknown_fields)]
struct DefaultInputSchema {}

/// An advertised schema: a client's arbitrary JSON Schema or the concrete default object.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum InputSchema<'a> {
    /// Closed default supplied when the caller declared no parameter schema.
    Default(DefaultInputSchema),
    /// Caller-authored JSON Schema, outside the gateway's vocabulary.
    Authored(&'a Value),
}

/// Builds the fixed empty object parameter schema.
fn default_input_schema() -> InputSchema<'static> {
    InputSchema::Default(DefaultInputSchema {})
}

/// Renders the IR tools into the upstream shape: the client's schema rides through, and a tool
/// declared without one receives the empty object schema.
fn render_tools(tools: &[IrTool]) -> Vec<UpstreamTool<'_>> {
    tools
        .iter()
        .map(|tool| UpstreamTool {
            name: &tool.name,
            description: tool.description.as_deref(),
            input_schema: tool
                .parameters
                .as_ref()
                .map(InputSchema::Authored)
                .unwrap_or_else(|| default_input_schema()),
        })
        .collect()
}

/// Renders the upstream Anthropic Messages payload: the body only — the URL, the headers, and the
/// streaming decision belong to the provider definition (`docs/design/providers.md`).
///
/// # Errors
///
/// Returns [`InvalidRequest`] when the IR carries a control the upstream has no slot for — tool
/// arguments that do not encode a JSON object, or a free-form JSON response format, which the
/// upstream cannot express.
pub(crate) fn render_request_body<'a>(
    ir: &'a IrRequest,
    is_streaming: bool,
) -> Result<RequestBody<'a>, InvalidRequest> {
    // The upstream expresses exactly one output-format constraint — a JSON schema under
    // `output_config.format`, beside the effort (the official `JSONOutputFormatParam` requires
    // both members) — and has no slot for free-form JSON: dropping the constraint would silently
    // return prose, so the mismatch fails explicitly instead. The OpenAI-style schema name has no
    // official member and is dropped (the Interactions codec's posture,
    // `docs/design/providers.md`).
    let output_format = match &ir.response_format {
        None | Some(IrResponseFormat::Text) => None,

        Some(IrResponseFormat::Json) => {
            return Err(InvalidRequest::field(
                "response_format",
                "The Anthropic Messages upstream expresses only a JSON schema response format",
            ));
        }

        Some(IrResponseFormat::JsonSchema { schema, .. }) => Some(JsonOutputFormat {
            kind: "json_schema",
            schema,
        }),
    };

    let tools = render_tools(&ir.tools);

    let tool_choice = ir
        .tool_choice
        .as_ref()
        .map(|tool_choice| match tool_choice {
            IrToolChoice::Auto => UpstreamToolChoice::Auto,
            IrToolChoice::None => UpstreamToolChoice::None,
            IrToolChoice::Required => UpstreamToolChoice::Any,
            IrToolChoice::Function { name } => UpstreamToolChoice::Tool { name },
        });

    let (effort, thinking) = match &ir.reasoning_effort {
        None => (None, None),

        Some(ReasoningLevel::None) => (None, Some(ThinkingDisabled { kind: "disabled" })),

        Some(level) => (Some(level.as_str()), None),
    };

    // The container rides when the request carries either member; a single-member object
    // serializes with the other omitted.
    let output_config = (effort.is_some() || output_format.is_some()).then_some(OutputConfig {
        effort,
        format: output_format,
    });

    Ok(RequestBody {
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
        compaction: None,
        context_management: None,
    })
}

/// Renders the native on-demand compaction body: the normal upstream payload with the
/// `compaction: {"type": "summarize"}` control beside it, `instructions` on the control when
/// the caller supplied some, and `max_tokens` overridden with the compaction output budget the
/// caller passes. When the request carries tool definitions — kept for the signed-prefix
/// stability native replay may require — the tool selection renders `none`, since a compaction
/// turn must never answer by calling a tool; the caller decides what the IR carries.
///
/// # Errors
///
/// Returns [`InvalidRequest`] exactly when [`render_request_body`] does.
pub(crate) fn render_on_demand_body<'a>(
    ir: &'a IrRequest,
    is_streaming: bool,
    instructions: Option<&'a str>,
    max_tokens: u64,
) -> Result<RequestBody<'a>, InvalidRequest> {
    let mut body = render_request_body(ir, is_streaming)?;

    body.max_tokens = max_tokens;
    body.compaction = Some(OnDemandControl {
        kind: "summarize",
        instructions,
    });

    if !ir.tools.is_empty() {
        body.tool_choice = Some(UpstreamToolChoice::None);
    }

    Ok(body)
}

/// Renders the native token-count request the definition's `count_tokens` operation dispatches:
/// the count endpoint's own body shape — model, system, messages, and tools, with no
/// generation-only control (no `max_tokens`, no `stream`, no output configuration).
///
/// # Errors
///
/// Returns [`InvalidRequest`] when the request's history does not render.
pub(crate) fn render_count_tokens_request(
    ir: &IrRequest,
) -> Result<CountTokensBody<'_>, InvalidRequest> {
    Ok(CountTokensBody {
        model: &ir.model,
        system: ir.instructions.as_deref(),
        messages: render_messages(ir)?,
        tools: render_tools(&ir.tools),
    })
}

/// The count endpoint's request body.
#[derive(Debug, Serialize)]
pub(crate) struct CountTokensBody<'a> {
    /// Upstream model name.
    model: &'a str,

    /// Concatenated system instructions, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,

    /// Strictly alternating user/assistant turns.
    messages: Vec<UpstreamMessage>,

    /// Advertised function tools, omitted when the request has none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<UpstreamTool<'a>>,
}

/// Renders the native threshold compaction body: the normal upstream payload with the
/// `context_management` member carrying the caller's vendor-shaped edit objects verbatim.
///
/// # Errors
///
/// Returns [`InvalidRequest`] exactly when [`render_request_body`] does.
pub(crate) fn render_threshold_body<'a>(
    ir: &'a IrRequest,
    is_streaming: bool,
    edits: &'a [crate::ir::compaction::ThresholdEdit],
) -> Result<RequestBody<'a>, InvalidRequest> {
    let mut body = render_request_body(ir, is_streaming)?;

    body.context_management = Some(ContextManagementBody { edits });

    Ok(body)
}

/// Groups IR messages into strictly alternating user/assistant turns through the shared role-run
/// accumulator: consecutive same-role IR messages merge into one run, and a turn whose rendered
/// content is empty is not emitted — [`render_message`] renders nothing to push for it. Well-formed
/// requests open with a user turn, which the merge preserves.
///
/// # Errors
///
/// Returns [`InvalidRequest`] when a tool call's arguments do not encode a JSON object.
fn render_messages(ir: &IrRequest) -> Result<Vec<UpstreamMessage>, InvalidRequest> {
    let mut runs = RoleRun::<UpstreamRole, OutBlock>::default();

    // Compaction state items splice into the turn their offset precedes: a
    // leading block (offset 0) opens the first rendered turn, a historical
    // block rides the assistant turn at its history position. A gateway
    // carrier must have been decoded and removed before any dispatch, and an
    // item the Messages wire cannot express is inaccessible state — never
    // silently discarded.
    if ir.state_items.iter().any(|item| item.is_gateway) {
        return Err(InvalidRequest::request(
            "a gateway compaction carrier reached the upstream render without preparation",
        ));
    }

    let mut state_items = ir.state_items.iter().peekable();

    for (message_index, message) in ir.messages.iter().enumerate() {
        let mut preceding_state = Vec::new();

        while let Some(item) = state_items.next_if(|item| item.offset <= message_index) {
            preceding_state.push(state_item_block(item)?);
        }

        let rendered = render_message(message)?;

        match (preceding_state.is_empty(), rendered) {
            (true, None) => {}

            (true, Some((role, blocks))) => runs.push(role, blocks),

            (false, None) => runs.push(UpstreamRole::Assistant, preceding_state),

            (false, Some((UpstreamRole::Assistant, blocks))) => {
                let mut content = preceding_state;

                content.extend(blocks);

                runs.push(UpstreamRole::Assistant, content);
            }

            (false, Some((role, blocks))) => {
                // A state item preceding a non-assistant turn renders as its
                // own assistant turn (the replay contract's leading shape),
                // followed by the turn the message renders.
                runs.push(UpstreamRole::Assistant, preceding_state);
                runs.push(role, blocks);
            }
        }
    }

    // State items past the last message ride one trailing assistant turn.
    if state_items.peek().is_some() {
        let mut trailing = Vec::new();

        while let Some(item) = state_items.next_if(|_| true) {
            trailing.push(state_item_block(item)?);
        }

        runs.push(UpstreamRole::Assistant, trailing);
    }

    Ok(runs
        .into_runs()
        .into_iter()
        .map(|(role, content)| UpstreamMessage { role, content })
        .collect())
}

/// Validates one opaque state item as Messages-family state and renders it
/// verbatim. A foreign family's item — an id-less native Responses compaction
/// item included — is native state this wire cannot express — inaccessible,
/// and rejected rather than dropped. Gateway carriers were rejected as the
/// preparation invariant violation before any item reached this render.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for an item the Messages wire cannot express.
fn state_item_block(item: &StateItem) -> Result<OutBlock, InvalidRequest> {
    if item.family == StateFamily::Messages {
        return Ok(OutBlock::Verbatim(item.item.clone()));
    }

    Err(InvalidRequest::request(
        "the request carries compaction state the Anthropic upstream cannot replay",
    ))
}

/// Renders one IR message into its turn side and blocks; [`None`] when the message renders no
/// content at all. IR tool messages fold into user turns carrying `tool_result` blocks, since
/// the API has no tool role.
///
/// Assistant turns hoist same-family reasoning blocks before tool calls — the API requires
/// thinking first — while text keeps its relative order among the remaining blocks. A
/// foreign-family reasoning part renders nothing: the official `ThinkingBlockParam` requires the
/// upstream's own `signature`, so no synthetic block can replay, and the API treats thinking
/// history as optional, making the drop its own degradation shape.
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
                    IrPart::Reasoning { opaque, .. } => {
                        // A replayable same-family thinking/redacted block replays verbatim —
                        // a thinking block needs its non-blank signature, which the API
                        // verifies, rejecting a modified block with a 400. Every other
                        // payload — foreign-family, or a signature-less thinking shape —
                        // carries no signature the API can verify, so nothing renders for
                        // it (see the function contract above).
                        if is_anthropic_block(opaque) {
                            reasoning.push(OutBlock::Verbatim(opaque.clone()));
                        }
                    }

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
                        input: Value::Object(crate::format::arguments_as_object(arguments)?),
                    })),

                    // Assistant turns carry no images or tool results on this API; the IR type
                    // system routes those into other turn sides.
                    IrPart::Image { .. } | IrPart::ToolOutput { .. } => {}
                }
            }

            // Replayable reasoning hoists before the turn's text and tool calls; foreign-family
            // reasoning rendered nothing.
            let mut content = reasoning;

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
