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
use crate::ir::{
    InvalidRequest, IrImage, IrMessage, IrPart, IrReasoningLevel, IrRequest, IrResponseFormat,
    IrRole, IrToolChoice,
};
use serde::Serialize;
use serde_json::Value;

/// Fixed synthesis constant rendered as `max_tokens` when the client sent no
/// `max_output_tokens`: the API requires the field, so the gateway documents this stand-in
/// rather than inventing a caller-specific budget (README §5.8).
const SYNTHESIZED_MAX_TOKENS: u64 = 16384;

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

    /// The effort and output-format container, present when the request carries either member;
    /// omitted when it carries neither (the `none` effort renders `thinking: disabled` instead).
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

    let (effort, thinking) = match &ir.reasoning_effort {
        None => (None, None),

        Some(IrReasoningLevel::None) => (None, Some(ThinkingDisabled { kind: "disabled" })),

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
    })
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
}
