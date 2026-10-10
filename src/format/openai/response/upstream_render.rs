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
    ir::{
        InvalidRequest, IrImage, IrPart, IrRequest, IrResponseFormat, IrRole, StateFamily,
        StateItem,
    },
};
use serde::Serialize;
use serde_json::Value;
use std::borrow::Cow;

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
    context_management: Option<&'a [crate::ir::compaction::AutomaticEntry]>,
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
    /// Terminal control requesting exactly one native compaction artifact.
    CompactionTrigger,

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

/// Renders an image into the Responses `image_url` string: a remote URL borrows verbatim, and
/// only inline data synthesizes its data URL — the inverse of [`IrImage::from_url`].
fn image_url(image: &IrImage) -> Cow<'_, str> {
    match image {
        IrImage::Url(url) => Cow::Borrowed(url),

        IrImage::Inline {
            mime_type,
            data_base64,
        } => Cow::Owned(format!("data:{mime_type};base64,{data_base64}")),
    }
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
        /// Inline data URL or remote `http`/`https` URL (`docs/reference/inference.md`), borrowed
        /// verbatim for a remote image.
        image_url: Cow<'a, str>,
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
    /// Reasoning effort string rendered verbatim from the IR, opaque to the gateway
    /// (`docs/reference/inference.md`).
    effort: &'a str,
}

/// Renders the upstream Responses payload: the body only — the URL, the headers, and the
/// streaming decision belong to the provider definition (`docs/design/providers.md`).
///
/// The `omit_max_output_tokens` render option drops the client-requested cap: the upstream
/// rejects the parameter as unsupported, and the model's own output bound applies instead.
///
/// Native automatic controls ride `context_management`; absence emits no compaction member.
///
/// # Errors
///
/// Returns [`InvalidRequest`] when a gateway carrier is present among the
/// state items — such an item must be decoded and removed during compaction
/// preparation, so its presence at render time is an invariant violation that
/// fails fast instead of forwarding gateway state upstream.
pub(crate) fn render_request_body_with_extensions<'a>(
    ir: &'a IrRequest,
    is_streaming: bool,
    render_options: &[RenderOption],
    context_management: Option<&'a [crate::ir::compaction::AutomaticEntry]>,
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
/// (`docs/design/compaction.md`).
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
/// Returns [`InvalidRequest`] as [`render_request_body_with_extensions`].
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
                            image_url: image_url(image),
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

    if ir.has_compaction_trigger {
        input.push(InputItem::Typed(TypedInputItem::CompactionTrigger));
    }
    Ok(input)
}

/// Splices one opaque state item into the rendered input, rejecting gateway
/// carriers as [`render_input_items`] documents and foreign-family native
/// state — an Anthropic compaction block included — as inaccessible state no
/// splice can express.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for a gateway carrier or a foreign-family item.
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

    if state.family != StateFamily::Responses {
        return Err(InvalidRequest::request(
            "the request carries compaction state the Responses upstream cannot replay",
        ));
    }

    input.push(InputItem::State(state.item.clone()));

    Ok(())
}

/// Renders an IR request into the plain upstream Responses payload.
fn render_body<'a>(
    ir: &'a IrRequest,
    is_streaming: bool,
    context_management: Option<&'a [crate::ir::compaction::AutomaticEntry]>,
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
