//! Upstream-request rendering for the Responses codec: renders an IR request into the
//! `openai_response` upstream payload.
//!
//! The rendered payload is a first-party type in the upstream wire shape, so field presence per
//! request is declared by `skip_serializing_if` rather than assembled conditionally.

use crate::{
    format::{
        RenderOption,
        openai::{ToolChoice, render_tool_choice},
    },
    ir::{InvalidRequest, IrPart, IrRequest, IrResponseFormat, IrRole, StateItem},
};
use serde::Serialize;
use serde_json::Value;

/// The upstream Responses request payload, shared by the Codex and compatible entry points.
#[derive(Debug, Serialize)]
pub(crate) struct RequestBody<'a> {
    /// Upstream model name.
    model: &'a str,

    /// Conversation input items in order.
    input: Vec<InputItem<'a>>,

    /// Whether the upstream streams its response.
    stream: bool,

    /// Always `false`: the gateway is stateless.
    store: bool,

    /// Concatenated system instructions, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,

    /// Advertised function tools, omitted when the request has none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool<'a>>,

    /// Tool selection restriction, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ToolChoice<NamedToolChoice<'a>>>,

    /// Sampling temperature, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,

    /// Nucleus sampling threshold, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,

    /// Output token budget, omitted when the request has none and always omitted on the Codex
    /// endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u64>,

    /// Text output controls, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<TextControls<'a>>,

    /// Reasoning configuration, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Reasoning<'a>>,

    /// Vendor-shaped automatic-compaction controls passed through to a native
    /// automatic upstream, supplied by the compaction executor; omitted unless
    /// one dispatch carries them. The entries are third-party wire fragments
    /// whose shapes the upstream defines.
    #[serde(skip_serializing_if = "Option::is_none")]
    context_management: Option<&'a [Value]>,
}

/// One conversation item of the upstream request.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum InputItem<'a> {
    /// A typed conversation item.
    Typed(TypedInputItem<'a>),

    /// A verbatim vendor-shaped state item — native compaction or encrypted
    /// reasoning state — replayed at its history position. Its shape is the
    /// vendor's, not the gateway's, so the item rides as the raw value it was
    /// parsed from.
    State(Value),
}

/// The typed conversation items of the upstream request.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TypedInputItem<'a> {
    /// A user or assistant message.
    Message {
        /// Participating role.
        role: MessageRole,

        /// The message content parts.
        content: Vec<MessageContentPart<'a>>,
    },

    /// An assistant tool invocation.
    FunctionCall {
        /// Identifier of the invocation, round-tripped to the matching output.
        call_id: &'a str,

        /// Name of the invoked function.
        name: &'a str,

        /// JSON-encoded arguments text.
        arguments: &'a str,

        /// Upstream thought signature that must be replayed on the next turn.
        #[serde(skip_serializing_if = "Option::is_none")]
        thought_signature: Option<&'a str>,
    },

    /// A tool result for a prior function call.
    FunctionCallOutput {
        /// Identifier of the invocation being answered.
        call_id: &'a str,

        /// Result content.
        output: &'a str,
    },
}

/// Role of a rendered message item.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum MessageRole {
    /// The human side of the conversation.
    User,

    /// The model side of the conversation.
    Assistant,
}

/// One content part of a rendered message item: the `input_*` parts a client turn carries, and the
/// `output_text` part a replayed assistant turn carries.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum MessageContentPart<'a> {
    /// Plain text input.
    InputText {
        /// The text content.
        text: &'a str,
    },

    /// An image reference.
    InputImage {
        /// Inline data URL or remote `http`/`https` URL (README §6.6).
        image_url: String,
    },

    /// Assistant output text.
    OutputText {
        /// The text content.
        text: &'a str,
    },
}

/// One advertised function tool in the flat Responses shape.
#[derive(Debug, Serialize)]
struct Tool<'a> {
    /// Tool kind, always `function`.
    #[serde(rename = "type")]
    tool_type: &'static str,

    /// Function name the model calls.
    name: &'a str,

    /// Human-readable description.
    description: Option<&'a str>,

    /// JSON Schema of the accepted arguments, absent for a function that takes none.
    parameters: Option<&'a Value>,
}

/// The named-function `tool_choice` form.
#[derive(Debug, Serialize)]
struct NamedToolChoice<'a> {
    /// Tool kind, always `function`.
    #[serde(rename = "type")]
    tool_type: &'static str,

    /// Required function name.
    name: &'a str,
}

/// Text output controls.
#[derive(Debug, Serialize)]
struct TextControls<'a> {
    /// Response format restriction.
    format: ResponseFormat<'a>,
}

/// A response format restriction in the upstream shape.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseFormat<'a> {
    /// Free-form text.
    Text,

    /// Any valid JSON value.
    JsonObject,

    /// JSON conforming to a named schema.
    JsonSchema {
        /// Schema name.
        name: &'a str,

        /// The JSON Schema itself, whose shape is the client's to define.
        schema: &'a Value,
    },
}

/// Reasoning configuration.
#[derive(Debug, Serialize)]
struct Reasoning<'a> {
    /// Reasoning effort string rendered verbatim from the IR, opaque to the gateway (README §6.4).
    effort: &'a str,
}

/// Renders the upstream Responses payload: the body only — the URL, the headers, and the
/// streaming decision belong to the provider definition (README §5.2).
///
/// The `omit_max_output_tokens` render option drops the client-requested cap: the upstream
/// rejects the parameter as unsupported, and the model's own output bound applies instead.
///
/// # Errors
///
/// Returns [`InvalidRequest`] when a gateway carrier is present among the
/// state items — such an item must be decoded and removed during compaction
/// preparation, so its presence at render time is an invariant violation that
/// fails fast instead of forwarding gateway state upstream.
pub(crate) fn render_request_body<'a>(
    ir: &'a IrRequest,
    is_streaming: bool,
    render_options: &[RenderOption],
) -> Result<RequestBody<'a>, InvalidRequest> {
    render_request_body_with_extensions(ir, is_streaming, render_options, None)
}

/// Renders the upstream Responses payload with the compaction executor's
/// extensions: `context_management` carries the vendor-shaped automatic
/// compaction controls a native automatic upstream receives; every other
/// dispatch passes [`None`], rendering no such member.
///
/// # Errors
///
/// Returns [`InvalidRequest`] as [`render_request_body`].
pub(crate) fn render_request_body_with_extensions<'a>(
    ir: &'a IrRequest,
    is_streaming: bool,
    render_options: &[RenderOption],
    context_management: Option<&'a [Value]>,
) -> Result<RequestBody<'a>, InvalidRequest> {
    let mut body = render_body(ir, is_streaming, context_management)?;

    if render_options.contains(&RenderOption::OmitMaxOutputTokens) {
        body.max_output_tokens = None;
    }

    Ok(body)
}

/// The upstream standalone compact request payload: the model, the spliced
/// input window, and the instructions — nothing else. The compact body has no
/// generation-only controls and no `stream` member, so the framing is always
/// non-streaming regardless of the definition's inference streaming mode
/// (README §6.8).
#[derive(Debug, Serialize)]
pub(crate) struct CompactUpstreamRequest<'a> {
    /// Upstream model name.
    model: &'a str,

    /// Conversation input items in order.
    input: Vec<InputItem<'a>>,

    /// Concatenated system instructions, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
}

/// Renders the upstream standalone compact payload from a compact request's
/// IR: the same input splicing as the ordinary render, under the compact
/// body's narrower shape.
///
/// # Errors
///
/// Returns [`InvalidRequest`] as [`render_request_body`].
pub(crate) fn render_compact_request_body(
    ir: &IrRequest,
) -> Result<CompactUpstreamRequest<'_>, InvalidRequest> {
    Ok(CompactUpstreamRequest {
        model: &ir.model,
        input: render_input_items(ir)?,
        instructions: ir.instructions.as_deref(),
    })
}

/// Renders the conversation input: the IR messages as typed items, with the
/// request's opaque state items spliced verbatim at their offsets.
///
/// A state item's offset is the number of IR messages rendered before it, so
/// an item of offset `k` lands after the first `k` messages — items of equal
/// offset go after those messages, in their own relative order — and items
/// beyond the last message close the array.
///
/// # Errors
///
/// Returns [`InvalidRequest`] when a state item is a gateway carrier: such an
/// item must be decoded and removed during compaction preparation, so its
/// presence at render time is an invariant violation that fails fast instead
/// of forwarding gateway state upstream.
fn render_input_items(ir: &IrRequest) -> Result<Vec<InputItem<'_>>, InvalidRequest> {
    let mut state_items = ir.state_items.iter();

    let mut next_state = state_items.next();

    let mut input: Vec<InputItem<'_>> =
        Vec::with_capacity(ir.messages.len() + ir.state_items.len());

    for (messages_rendered, message) in ir.messages.iter().enumerate() {
        while next_state.is_some_and(|state| state.offset <= messages_rendered) {
            push_state_item(&mut input, next_state.take())?;

            next_state = state_items.next();
        }

        match message.role {
            IrRole::User => {
                let content: Vec<MessageContentPart<'_>> = message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        IrPart::Text { text } => Some(MessageContentPart::InputText { text }),

                        IrPart::Image { image } => Some(MessageContentPart::InputImage {
                            image_url: image.to_url(),
                        }),

                        // The IR type system routes only text and images into user content.
                        _ => None,
                    })
                    .collect();

                input.push(InputItem::Typed(TypedInputItem::Message {
                    role: MessageRole::User,
                    content,
                }));
            }

            IrRole::Assistant => {
                for part in &message.parts {
                    match part {
                        IrPart::Text { text } => {
                            input.push(InputItem::Typed(TypedInputItem::Message {
                                role: MessageRole::Assistant,
                                content: vec![MessageContentPart::OutputText { text }],
                            }));
                        }

                        IrPart::ToolCall {
                            call_id,
                            name,
                            arguments,
                            thought_signature,
                        } => {
                            input.push(InputItem::Typed(TypedInputItem::FunctionCall {
                                call_id,
                                name,
                                arguments,
                                thought_signature: thought_signature.as_deref(),
                            }));
                        }

                        _ => {}
                    }
                }
            }

            IrRole::Tool => {
                for part in &message.parts {
                    if let IrPart::ToolOutput {
                        call_id, content, ..
                    } = part
                    {
                        input.push(InputItem::Typed(TypedInputItem::FunctionCallOutput {
                            call_id,
                            output: content,
                        }));
                    }
                }
            }
        }
    }

    // Items beyond the last message close the array.
    while let Some(state) = next_state {
        push_state_item(&mut input, Some(state))?;

        next_state = state_items.next();
    }

    Ok(input)
}

/// Splices one opaque state item into the rendered input, rejecting gateway
/// carriers as [`render_input_items`] documents.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for a gateway carrier.
fn push_state_item<'a>(
    input: &mut Vec<InputItem<'a>>,
    state: Option<&'a StateItem>,
) -> Result<(), InvalidRequest> {
    let Some(state) = state else {
        return Ok(());
    };

    if state.is_gateway {
        return Err(InvalidRequest::request(
            "a gateway compaction carrier reached the upstream render without preparation",
        ));
    }

    input.push(InputItem::State(state.item.clone()));

    Ok(())
}

/// Renders an IR request into the plain upstream Responses payload.
fn render_body<'a>(
    ir: &'a IrRequest,
    is_streaming: bool,
    context_management: Option<&'a [Value]>,
) -> Result<RequestBody<'a>, InvalidRequest> {
    let input = render_input_items(ir)?;

    let tools = ir
        .tools
        .iter()
        .map(|tool| Tool {
            tool_type: "function",
            name: &tool.name,
            description: tool.description.as_deref(),
            parameters: tool.parameters.as_ref(),
        })
        .collect();

    let tool_choice = ir.tool_choice.as_ref().map(|tool_choice| {
        render_tool_choice(tool_choice, |name| NamedToolChoice {
            tool_type: "function",
            name,
        })
    });

    let text = ir
        .response_format
        .as_ref()
        .map(|response_format| TextControls {
            format: match response_format {
                IrResponseFormat::Text => ResponseFormat::Text,
                IrResponseFormat::Json => ResponseFormat::JsonObject,

                IrResponseFormat::JsonSchema { name, schema } => {
                    ResponseFormat::JsonSchema { name, schema }
                }
            },
        });

    Ok(RequestBody {
        model: &ir.model,
        input,
        stream: is_streaming,
        store: false,
        instructions: ir.instructions.as_deref(),
        tools,
        tool_choice,
        temperature: ir.temperature,
        top_p: ir.top_p,
        max_output_tokens: ir.max_output_tokens,
        text,
        reasoning: ir.reasoning_effort.as_ref().map(|level| Reasoning {
            effort: level.as_str(),
        }),
        context_management,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CompactUpstreamRequest, InputItem, TypedInputItem, render_compact_request_body,
        render_request_body, render_request_body_with_extensions,
    };
    use crate::{
        format::RenderOption,
        ir::{IrReasoningLevel, IrRequest, IrRole, IrToolChoice, StateItem},
    };
    use serde_json::{Value, json};

    /// Renders one upstream body for the given effort level.
    ///
    /// # Panics
    ///
    /// Panics if the rendered body fails to serialize or fails to render,
    /// neither of which can occur for the fixture.
    fn render_body(reasoning_effort: Option<IrReasoningLevel>) -> Value {
        let ir = IrRequest {
            state_items: Vec::new(),
            model: "m".to_string(),
            reasoning_effort,
            ..IrRequest::default()
        };

        let body = render_request_body(&ir, false, &[]);

        #[expect(
            clippy::expect_used,
            reason = "the rendered payload is under test; a failing render is the failure"
        )]
        serde_json::to_value(body.expect("the fixture renders")).expect("the body serializes")
    }

    /// Verifies the Responses effort slot of the closed-vocabulary contract (README §6.4): every
    /// vocabulary value renders verbatim, and an absent effort renders no reasoning object at all.
    ///
    /// # Panics
    ///
    /// Panics if a rendered slot diverges from the contract.
    #[test]
    fn renders_the_effort_level_verbatim_or_not_at_all() {
        for (level, effort) in IrReasoningLevel::VOCABULARY {
            let body = render_body(Some(level));

            assert_eq!(
                body.get("reasoning")
                    .and_then(|reasoning| reasoning.get("effort"))
                    .and_then(Value::as_str),
                Some(effort)
            );
        }

        assert!(render_body(None).get("reasoning").is_none());
    }

    /// Verifies the `tool_choice` outbound contract: the three shared modes render as bare mode
    /// strings, and the named function renders in this codec's flat `{"type": "function",
    /// "name": ...}` shape.
    ///
    /// # Panics
    ///
    /// Panics if a rendered choice diverges from the contract.
    #[test]
    fn maps_the_tool_choice_modes_and_the_named_function() {
        for (choice, rendered) in [
            (IrToolChoice::Auto, json!("auto")),
            (IrToolChoice::None, json!("none")),
            (IrToolChoice::Required, json!("required")),
            (
                IrToolChoice::Function {
                    name: "search".to_string(),
                },
                json!({"type": "function", "name": "search"}),
            ),
        ] {
            let ir = IrRequest {
                state_items: Vec::new(),
                tool_choice: Some(choice),
                ..IrRequest::default()
            };

            #[expect(
                clippy::expect_used,
                reason = "the rendered payload is under test; a failing render is the failure"
            )]
            let body = serde_json::to_value(
                render_request_body(&ir, false, &[]).expect("the fixture renders"),
            )
            .expect("the body serializes");

            assert_eq!(body.get("tool_choice"), Some(&rendered));
        }
    }

    /// Builds a one-line user message IR with the given state items.
    fn ir_with_state_items(state_items: Vec<StateItem>) -> IrRequest {
        IrRequest {
            state_items,
            model: "m".to_string(),
            messages: vec![
                crate::ir::IrMessage {
                    role: IrRole::User,
                    parts: vec![crate::ir::IrPart::Text {
                        text: "first".to_string(),
                    }],
                },
                crate::ir::IrMessage {
                    role: IrRole::User,
                    parts: vec![crate::ir::IrPart::Text {
                        text: "second".to_string(),
                    }],
                },
            ],
            ..IrRequest::default()
        }
    }

    /// Renders the fixture IR and returns the rendered input array.
    ///
    /// # Panics
    ///
    /// Panics if the render fails or the body fails to serialize.
    fn rendered_input(ir: &IrRequest) -> Vec<Value> {
        #[expect(
            clippy::expect_used,
            reason = "the rendered payload is under test; a failing render is the failure"
        )]
        let body =
            serde_json::to_value(render_request_body(ir, false, &[]).expect("the fixture renders"))
                .expect("the body serializes");

        #[expect(
            clippy::expect_used,
            reason = "the fixture body always carries an input array"
        )]
        body.get("input")
            .and_then(Value::as_array)
            .expect("the body carries an input array")
            .clone()
    }

    /// State items splice at their exact offsets: an item of offset `k` lands
    /// after the first `k` messages, items of equal offset keep their relative
    /// order, and items beyond the last message close the array.
    ///
    /// # Panics
    ///
    /// Panics if the spliced positions diverge from the offset contract.
    #[test]
    fn state_items_splice_at_their_exact_offsets() {
        let item = |offset: usize, text: &str, is_gateway: bool| StateItem {
            offset,
            item: json!({"type": "reasoning", "id": text, "encrypted_content": text}),
            is_gateway,
        };

        let ir = ir_with_state_items(vec![
            item(0, "head", false),
            item(1, "between", false),
            item(1, "between-b", false),
            item(2, "tail", false),
        ]);

        let input = rendered_input(&ir);

        let positions: Vec<Option<&str>> = input
            .iter()
            .map(|item| item.get("id").and_then(Value::as_str))
            .collect();

        assert_eq!(
            positions,
            vec![
                Some("head"),
                None, // the first message
                Some("between"),
                Some("between-b"),
                None, // the second message
                Some("tail"),
            ],
        );

        // Every message item is present in order between the state items.
        let message_texts: Vec<&str> = input
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
            .filter_map(|item| item.pointer("/content/0/text").and_then(Value::as_str))
            .collect();

        assert_eq!(message_texts, vec!["first", "second"]);
    }

    /// A gateway carrier among the state items is an invariant violation: the
    /// render fails instead of forwarding gateway state upstream.
    ///
    /// # Panics
    ///
    /// Panics if the gateway carrier renders.
    #[test]
    fn a_gateway_carrier_fails_the_render() {
        let ir = ir_with_state_items(vec![StateItem {
            offset: 0,
            item: json!({"type": "compaction", "encrypted_content": "sealed"}),
            is_gateway: true,
        }]);

        #[expect(
            clippy::expect_used,
            reason = "the gateway carrier is constructed to be rejected"
        )]
        let error = render_request_body(&ir, false, &[])
            .expect_err("a gateway carrier must fail the render");

        assert!(error.to_string().contains("gateway compaction carrier"));
    }

    /// The compact upstream body carries exactly the compact shape — model,
    /// spliced input, instructions — with no `stream` and no `store` member,
    /// whatever the inference streaming mode is (README §6.8).
    ///
    /// # Panics
    ///
    /// Panics if the rendered body diverges from the compact shape.
    #[test]
    fn the_compact_body_renders_non_streaming_with_spliced_state() {
        let ir = ir_with_state_items(vec![StateItem {
            offset: 1,
            item: json!({"type": "compaction", "id": "cmp_1", "encrypted_content": "state"}),
            is_gateway: false,
        }]);

        let ir = IrRequest {
            instructions: Some("summarize".to_string()),
            ..ir
        };

        #[expect(
            clippy::expect_used,
            reason = "the rendered payload is under test; a failing render is the failure"
        )]
        let body: CompactUpstreamRequest<'_> =
            render_compact_request_body(&ir).expect("the fixture renders");

        #[expect(
            clippy::expect_used,
            reason = "the rendered payload is under test; a failing render is the failure"
        )]
        let rendered = serde_json::to_value(&body).expect("the body serializes");

        assert_eq!(rendered.get("model"), Some(&json!("m")));

        assert_eq!(rendered.get("instructions"), Some(&json!("summarize")));

        assert!(rendered.get("stream").is_none());

        assert!(rendered.get("store").is_none());

        assert_eq!(
            rendered.get("input"),
            Some(&json!([
                {"type": "message", "role": "user", "content": [
                    {"type": "input_text", "text": "first"},
                ]},
                {"type": "compaction", "id": "cmp_1", "encrypted_content": "state"},
                {"type": "message", "role": "user", "content": [
                    {"type": "input_text", "text": "second"},
                ]},
            ])),
        );
    }

    /// `context_management` renders onto the body only when the executor
    /// supplied entries, verbatim in declared order; the plain render carries
    /// no such member.
    ///
    /// # Panics
    ///
    /// Panics if the rendered bodies diverge from the passthrough contract.
    #[test]
    fn context_management_passes_through_only_when_supplied() {
        let ir = ir_with_state_items(Vec::new());

        let entries = [json!({"type": "compaction", "compact_threshold": 50_000})];

        #[expect(
            clippy::expect_used,
            reason = "the rendered payload is under test; a failing render is the failure"
        )]
        let extended = serde_json::to_value(
            render_request_body_with_extensions(&ir, false, &[], Some(&entries))
                .expect("the fixture renders"),
        )
        .expect("the body serializes");

        assert_eq!(
            extended.get("context_management"),
            Some(&json!([{"type": "compaction", "compact_threshold": 50_000}])),
        );

        #[expect(
            clippy::expect_used,
            reason = "the rendered payload is under test; a failing render is the failure"
        )]
        let plain = serde_json::to_value(
            render_request_body(&ir, false, &[]).expect("the fixture renders"),
        )
        .expect("the body serializes");

        assert!(plain.get("context_management").is_none());
    }

    /// The typed conversation items render in the official shape under the
    /// untagged wrapper exactly as before the state-item vocabulary existed:
    /// message, function call, and function call output items are unchanged.
    ///
    /// # Panics
    ///
    /// Panics if a rendered item diverges from its documented shape.
    #[test]
    fn typed_items_render_their_official_shapes() {
        let items = [
            InputItem::Typed(TypedInputItem::Message {
                role: super::MessageRole::User,
                content: vec![super::MessageContentPart::InputText { text: "hi" }],
            }),
            InputItem::State(json!({"type": "compaction", "id": "cmp_1"})),
        ];

        #[expect(
            clippy::expect_used,
            reason = "the rendered payload is under test; a failing render is the failure"
        )]
        let rendered = serde_json::to_value(&items).expect("the items serialize");

        assert_eq!(
            rendered,
            json!([
                {"type": "message", "role": "user", "content": [
                    {"type": "input_text", "text": "hi"},
                ]},
                {"type": "compaction", "id": "cmp_1"},
            ]),
        );
    }

    /// The render option table still applies through the extension entry
    /// point: `omit_max_output_tokens` drops the cap on a body carrying one.
    ///
    /// # Panics
    ///
    /// Panics if the rendered bodies diverge from the option contract.
    #[test]
    fn render_options_apply_through_the_extension_entry_point() {
        let ir = IrRequest {
            model: "m".to_string(),
            max_output_tokens: Some(64),
            ..IrRequest::default()
        };

        #[expect(
            clippy::expect_used,
            reason = "the rendered payload is under test; a failing render is the failure"
        )]
        let omitted = serde_json::to_value(
            render_request_body_with_extensions(
                &ir,
                false,
                &[RenderOption::OmitMaxOutputTokens],
                None,
            )
            .expect("the fixture renders"),
        )
        .expect("the body serializes");

        assert!(omitted.get("max_output_tokens").is_none());
    }
}
