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
//! (README §6.4).

use super::{ImageSourceOut, OutBlock, TypedBlock, is_anthropic_block};
use crate::{
    format::RoleRun,
    ir::{
        InvalidRequest, IrImage, IrMessage, IrPart, IrRequest, IrResponseFormat, IrRole, IrTool,
        IrToolChoice,
    },
    vocabulary::ReasoningLevel,
};
use serde::Serialize;
use serde_json::Value;

/// Fixed synthesis constant rendered as `max_tokens` when the client sent no
/// `max_output_tokens`: the API requires the field, so the gateway documents this stand-in
/// rather than inventing a caller-specific budget (README §5.8).
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

    /// Native off spelling, rendered only for the `none` effort level (README §6.4).
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
pub(crate) struct UpstreamMessage {
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
pub(crate) struct UpstreamTool<'a> {
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
/// streaming decision belong to the provider definition (README §5.2).
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
    // official member and is dropped (the Interactions codec's posture, README §5.7).
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
    pub(crate) model: &'a str,

    /// Concatenated system instructions, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) system: Option<&'a str>,

    /// Strictly alternating user/assistant turns.
    pub(crate) messages: Vec<UpstreamMessage>,

    /// Advertised function tools, omitted when the request has none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) tools: Vec<UpstreamTool<'a>>,
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
            preceding_state.push(state_item_block(&item.item)?);
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
            trailing.push(state_item_block(&item.item)?);
        }

        runs.push(UpstreamRole::Assistant, trailing);
    }

    Ok(runs
        .into_runs()
        .into_iter()
        .map(|(role, content)| UpstreamMessage { role, content })
        .collect())
}

/// Validates one opaque state item as an Anthropic compaction block and
/// renders it verbatim. Anything else is native state this wire cannot
/// express — inaccessible, and rejected rather than dropped.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for an item the Messages wire cannot express.
fn state_item_block(item: &Value) -> Result<OutBlock, InvalidRequest> {
    // The family check reads the member vocabulary: an Anthropic compaction
    // block carries only the block schema's own members, while a foreign
    // family's compaction item (the Responses item's `id`/
    // `encrypted_content`-only shape) carries members this wire never
    // declares. A vendor block may carry vendor extensions, so the check is
    // the inverse one: a known foreign-family member marks it foreign.
    const FOREIGN_MEMBERS: [&str; 1] = ["id"];

    if let Some(object) = item.as_object()
        && object.get("type").and_then(Value::as_str) == Some("compaction")
        && FOREIGN_MEMBERS
            .iter()
            .all(|member| !object.contains_key(*member))
    {
        return Ok(OutBlock::Verbatim(item.clone()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::at;
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
        let body = render_request_body(ir, is_streaming).expect("the fixture request renders");

        #[expect(
            clippy::expect_used,
            reason = "the rendered body is under test; a failing render is the failure"
        )]
        serde_json::to_value(&body).expect("the rendered body serializes")
    }

    /// Builds the IR of one alternating conversation with a tool round-trip.
    fn conversation_ir() -> IrRequest {
        IrRequest {
            state_items: Vec::new(),
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
    /// the tool message folds into a user turn carrying its `is_error` result, a cross-family
    /// reasoning part renders nothing, and instructions render as the system string.
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

        // The cross-family reasoning part (a null opaque payload, with even a foreign signature)
        // drops; text keeps its order ahead of the tool call.
        assert_eq!(
            at(&body, "/messages/1/content"),
            &json!([
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
            state_items: Vec::new(),
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
            state_items: Vec::new(),
            max_output_tokens: Some(64),
            ..ir.clone()
        };

        assert_eq!(
            at(&upstream_body(&with_budget, true), "/max_tokens"),
            &json!(64)
        );

        let empty_assistant = IrRequest {
            state_items: Vec::new(),
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
            state_items: Vec::new(),
            model: "m".to_string(),
            messages: vec![IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: "hi".to_string(),
                }],
            }],
            ..IrRequest::default()
        };

        for (level, effort) in ReasoningLevel::VOCABULARY {
            // `none` renders as the native disabled spelling, asserted separately below.
            if level == ReasoningLevel::None {
                continue;
            }

            let ir = IrRequest {
                state_items: Vec::new(),
                reasoning_effort: Some(level),
                ..base.clone()
            };

            let body = upstream_body(&ir, false);

            assert_eq!(at(&body, "/output_config"), &json!({"effort": effort}));
            assert!(body.get("thinking").is_none());
        }

        let ir = IrRequest {
            state_items: Vec::new(),
            reasoning_effort: Some(ReasoningLevel::None),
            ..base.clone()
        };

        let body = upstream_body(&ir, false);

        assert_eq!(at(&body, "/thinking"), &json!({"type": "disabled"}));
        assert!(body.get("output_config").is_none());

        let body = upstream_body(&base, false);

        assert!(body.get("output_config").is_none());
        assert!(body.get("thinking").is_none());
    }

    /// Replays a same-family opaque thinking block verbatim and maps the four tool-choice modes.
    ///
    /// # Panics
    ///
    /// Panics if the replay or the mappings diverge from the contract.
    #[test]
    fn replays_opaque_blocks_and_maps_tool_choice() {
        let opaque: Value = json!({
            "type": "thinking",
            "thinking": "upstream text",
            "signature": "upstream-sig",
        });

        let ir = IrRequest {
            state_items: Vec::new(),
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
                state_items: Vec::new(),
                tool_choice: Some(choice),
                ..IrRequest::default()
            };

            // The choice mapping needs no messages; only the choice is inspected.
            #[expect(
                clippy::expect_used,
                reason = "the tool choice is constructed to render"
            )]
            let rendered_body = render_request_body(&ir, false).expect("the tool choice renders");

            #[expect(
                clippy::expect_used,
                reason = "the rendered body is under test; a failing render is the failure"
            )]
            let value = serde_json::to_value(&rendered_body).expect("the body serializes");

            assert_eq!(at(&value, "/tool_choice"), &rendered);
        }
    }

    /// Renders a JSON-schema response format into `output_config.format` — the official
    /// `JSONOutputFormatParam`, whose `type` and `schema` members are both required and which
    /// carries no slot for the OpenAI-style schema name — while free-form JSON, which the
    /// upstream cannot express, fails explicitly naming the field, and plain text renders no
    /// format member at all.
    ///
    /// # Panics
    ///
    /// Panics if the rendered format or the rejection diverges from the contract.
    #[test]
    fn renders_the_json_schema_format_and_rejects_free_form_json() {
        let schema = json!({"type": "object", "properties": {"city": {"type": "string"}}});

        let ir = IrRequest {
            state_items: Vec::new(),
            model: "m".to_string(),
            response_format: Some(IrResponseFormat::JsonSchema {
                name: "shape".to_string(),
                schema: schema.clone(),
            }),
            ..IrRequest::default()
        };

        let body = upstream_body(&ir, false);

        assert_eq!(
            at(&body, "/output_config/format"),
            &json!({"type": "json_schema", "schema": schema}),
        );

        let ir = IrRequest {
            state_items: Vec::new(),
            model: "m".to_string(),
            response_format: Some(IrResponseFormat::Json),
            ..IrRequest::default()
        };

        #[expect(
            clippy::expect_used,
            reason = "the response format is constructed to be rejected"
        )]
        let error = render_request_body(&ir, false)
            .expect_err("a free-form JSON response format has no Anthropic slot");

        assert_eq!(error.param.as_deref(), Some("response_format"));

        assert_eq!(
            error.to_string(),
            "The Anthropic Messages upstream expresses only a JSON schema response format"
        );

        // Plain text is the absence of a constraint: no output format renders.
        let ir = IrRequest {
            state_items: Vec::new(),
            model: "m".to_string(),
            response_format: Some(IrResponseFormat::Text),
            ..IrRequest::default()
        };

        assert!(upstream_body(&ir, false).get("output_config").is_none());
    }

    /// Drops a cross-family reasoning part — minted the way the Interactions inbound parser mints
    /// its thought steps, the whole step JSON as the opaque payload — instead of synthesizing a
    /// thinking block the official API would reject: its `ThinkingBlockParam` requires the
    /// upstream's own `signature`, so a foreign block cannot replay, and the API treats thinking
    /// history as optional. The rest of the turn renders intact, and an assistant turn whose only
    /// part is foreign reasoning renders no turn.
    ///
    /// # Panics
    ///
    /// Panics if the rendered turn diverges from the drop contract.
    #[test]
    fn drops_foreign_reasoning_and_renders_the_turn_intact() {
        let ir = IrRequest {
            state_items: Vec::new(),
            model: "m".to_string(),
            max_output_tokens: Some(64),
            messages: vec![
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: "hi".to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![
                        IrPart::Reasoning {
                            text: "interactions thought".to_string(),
                            // The step's optional signature rides along, but it is a foreign
                            // family's signature the Messages API cannot verify.
                            signature: Some("interactions-sig".to_string()),
                            opaque: json!({
                                "type": "thought",
                                "summary": [
                                    {"type": "text", "text": "interactions thought"},
                                ],
                                "signature": "interactions-sig",
                            }),
                        },
                        IrPart::Text {
                            text: "answer".to_string(),
                        },
                        IrPart::ToolCall {
                            call_id: "call_1".to_string(),
                            name: "ping".to_string(),
                            arguments: r#"{"q":"x"}"#.to_string(),
                            thought_signature: None,
                        },
                    ],
                },
            ],
            ..IrRequest::default()
        };

        let body = upstream_body(&ir, false);

        assert_eq!(
            at(&body, "/messages/1/content"),
            &json!([
                {"type": "text", "text": "answer"},
                {
                    "type": "tool_use",
                    "id": "call_1",
                    "name": "ping",
                    "input": {"q": "x"},
                },
            ]),
        );

        // A turn whose only part is foreign reasoning renders no content, so no turn.
        let reasoning_only = IrRequest {
            state_items: Vec::new(),
            messages: vec![
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: "hi".to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![IrPart::Reasoning {
                        text: "lost".to_string(),
                        signature: None,
                        opaque: json!({"type": "thought", "summary": []}),
                    }],
                },
            ],
            ..ir
        };

        let body = upstream_body(&reasoning_only, false);

        assert_eq!(at(&body, "/messages").as_array().map(Vec::len), Some(1));
        assert_eq!(at(&body, "/messages/0/role"), &json!("user"));
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
            state_items: Vec::new(),
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

    /// Renders the native on-demand compaction body: the `compaction` control beside the
    /// ordinary payload with its `instructions` member when supplied, `max_tokens` overridden
    /// with the compaction output budget, and the tool selection forced to `none` when the
    /// request carries tool definitions for prefix stability.
    ///
    /// # Panics
    ///
    /// Panics if the rendered body diverges from the native on-demand shape.
    #[test]
    fn renders_the_native_on_demand_body() {
        let ir = IrRequest {
            state_items: Vec::new(),
            model: "claude-test".to_string(),
            max_output_tokens: Some(999),
            reasoning_effort: Some(ReasoningLevel::Low),
            tools: vec![crate::ir::IrTool {
                name: "search".to_string(),
                description: None,
                parameters: Some(json!({"type": "object"})),
            }],
            tool_choice: Some(IrToolChoice::Required),
            messages: vec![IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: "history".to_string(),
                }],
            }],
            ..IrRequest::default()
        };

        #[expect(
            clippy::expect_used,
            reason = "the fixture request renders; a failing render is the failure"
        )]
        let body = render_on_demand_body(&ir, true, Some("focus on decisions"), 4096)
            .expect("the on-demand body renders");

        #[expect(
            clippy::expect_used,
            reason = "the rendered body is under test; a failing render is the failure"
        )]
        let value = serde_json::to_value(&body).expect("the rendered body serializes");

        assert_eq!(
            at(&value, "/compaction"),
            &json!({"type": "summarize", "instructions": "focus on decisions"})
        );
        assert_eq!(at(&value, "/max_tokens"), &json!(4096));
        assert_eq!(at(&value, "/stream"), &json!(true));
        // The tool definitions ride for prefix stability; the selection is forced off.
        assert_eq!(at(&value, "/tool_choice"), &json!({"type": "none"}));
        assert_eq!(
            value
                .get("tools")
                .and_then(|tools| tools.as_array())
                .map(|tools| tools.len()),
            Some(1),
        );
        // Everything else renders ordinarily.
        assert_eq!(at(&value, "/output_config"), &json!({"effort": "low"}));

        // No instructions: the control carries the type alone, no null member.
        #[expect(clippy::expect_used, reason = "the instruction-less request renders")]
        let body =
            render_on_demand_body(&ir, false, None, 100).expect("the on-demand body renders");

        #[expect(
            clippy::expect_used,
            reason = "the rendered body is under test; a failing render is the failure"
        )]
        let value = serde_json::to_value(&body).expect("the rendered body serializes");

        assert_eq!(at(&value, "/compaction"), &json!({"type": "summarize"}));
        assert_eq!(at(&value, "/max_tokens"), &json!(100));
    }

    /// Renders the native threshold compaction body: the `context_management` member carries
    /// the caller's vendor-shaped edit objects verbatim, and the base payload is unchanged.
    ///
    /// # Panics
    ///
    /// Panics if the rendered body diverges from the native threshold shape.
    #[test]
    fn renders_the_native_threshold_body() {
        let ir = IrRequest {
            state_items: Vec::new(),
            model: "claude-test".to_string(),
            messages: vec![IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: "history".to_string(),
                }],
            }],
            ..IrRequest::default()
        };

        let edits = vec![
            json!({
                "type": "compact_20260112",
                "trigger": {"type": "input_tokens", "value": 150_000},
                "pause_after_compaction": true,
            }),
            json!({"type": "compact_20260112", "instructions": "keep the tools"}),
        ];

        #[expect(
            clippy::expect_used,
            reason = "the fixture request renders; a failing render is the failure"
        )]
        let concrete_edits = serde_json::from_value::<Vec<crate::ir::compaction::ThresholdEdit>>(
            serde_json::json!(edits),
        )
        .expect("threshold controls parse");
        #[expect(
            clippy::expect_used,
            reason = "the fixture request renders; a failing render is the failure"
        )]
        let body =
            render_threshold_body(&ir, false, &concrete_edits).expect("the threshold body renders");

        #[expect(
            clippy::expect_used,
            reason = "the rendered body is under test; a failing render is the failure"
        )]
        let value = serde_json::to_value(&body).expect("the rendered body serializes");

        assert_eq!(at(&value, "/context_management"), &json!({"edits": edits}));
        assert!(value.get("compaction").is_none());
        assert_eq!(at(&value, "/max_tokens"), &json!(SYNTHESIZED_MAX_TOKENS));
    }

    /// A leading compaction state item renders as the stream-opening assistant
    /// block: the cross-protocol restore places a wrapped Claude block first
    /// regardless of the source protocol's own placement rule.
    ///
    /// # Panics
    ///
    /// Panics if the rendered messages diverge from the leading-block shape.
    #[test]
    fn splices_a_leading_state_item_as_the_first_assistant_block() {
        let block = json!({
            "type": "compaction",
            "content": "the summary",
            "signature": "sig",
        });

        let mut ir = conversation_ir();

        ir.state_items.push(crate::ir::StateItem {
            offset: 0,
            item: block,
            is_gateway: false,
        });

        let body = upstream_body(&ir, false);

        assert_eq!(
            at(&body, "/messages/0"),
            &json!({
                "role": "assistant",
                "content": [{"type": "compaction", "content": "the summary", "signature": "sig"}],
            }),
        );

        assert_eq!(at(&body, "/messages/1/role"), &json!("user"));
    }

    /// A historical state item rides the assistant turn at its offset, ahead
    /// of that turn's own blocks.
    ///
    /// # Panics
    ///
    /// Panics if the rendered messages diverge from the historical shape.
    #[test]
    fn splices_a_historical_state_item_into_its_turn() {
        let block = json!({"type": "compaction", "content": null});

        let mut ir = conversation_ir();

        ir.messages.insert(
            1,
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![IrPart::Text {
                    text: "mid".to_string(),
                }],
            },
        );

        ir.state_items.push(crate::ir::StateItem {
            offset: 1,
            item: block,
            is_gateway: false,
        });

        let body = upstream_body(&ir, false);

        assert_eq!(
            at(&body, "/messages/1"),
            &json!({
                "role": "assistant",
                "content": [
                    {"type": "compaction", "content": Option::<String>::None},
                    {"type": "text", "text": "mid"},
                ],
            }),
        );
    }

    /// A state item the Messages wire cannot express is inaccessible state:
    /// the render rejects it rather than dropping it.
    ///
    /// # Panics
    ///
    /// Panics if a foreign-shaped item renders.
    #[test]
    fn rejects_an_inexpressible_state_item() {
        let mut ir = conversation_ir();

        ir.state_items.push(crate::ir::StateItem {
            offset: 0,
            item: json!({"type": "compaction", "id": "x", "encrypted_content": "opaque"}),
            is_gateway: false,
        });

        assert!(
            render_request_body(&ir, false).is_err(),
            "a Responses-shaped window cannot splice into the Messages wire"
        );
    }

    /// The base render never emits a compaction member — it is the stripped form the
    /// compaction executor uses for continuation calls.
    ///
    /// # Panics
    ///
    /// Panics if the base body carries either compaction member.
    #[test]
    fn the_base_render_emits_no_compaction_members() {
        let body = upstream_body(&conversation_ir(), false);

        assert!(body.get("compaction").is_none());
        assert!(body.get("context_management").is_none());
    }
}
