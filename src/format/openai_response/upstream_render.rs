//! Upstream-request rendering for the Responses codec: renders an IR request into the
//! `openai_response` upstream payload.
//!
//! The rendered payload is a first-party type in the upstream wire shape, so field presence per
//! request is declared by `skip_serializing_if` rather than assembled conditionally.

use crate::ir::{IrPart, IrRequest, IrResponseFormat, IrRole, IrToolChoice};
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
    tool_choice: Option<ToolChoice<'a>>,

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
    reasoning: Option<Reasoning>,
}

/// One conversation item of the upstream request.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum InputItem<'a> {
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

/// A `tool_choice` restriction in the upstream shape: a bare mode string or a named-function
/// object.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ToolChoice<'a> {
    /// The mode-string form.
    Mode(ToolChoiceMode),

    /// The named-function form.
    Function(NamedToolChoice<'a>),
}

/// The `tool_choice` mode strings.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ToolChoiceMode {
    /// The model decides freely.
    Auto,

    /// The model may not call tools.
    None,

    /// The model must call a tool.
    Required,
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
struct Reasoning {
    /// Canonical reasoning level rendered as the OpenAI effort string.
    effort: &'static str,
}

/// Renders the Codex-flavored upstream request URL and payload: streaming is forced on (README §5.1
/// invariant 2) and `max_output_tokens` is omitted — the ChatGPT Codex backend rejects it as an
/// unsupported parameter, and the model's own output bound applies instead of the client's cap.
///
/// `url` is the fixed Codex inference endpoint; the provider layer owns endpoint knowledge, so it
/// is threaded in rather than owned by the codec.
pub(crate) fn render_codex_request<'a>(ir: &'a IrRequest, url: &str) -> (String, RequestBody<'a>) {
    let mut body = render_request_body(ir, true);

    body.max_output_tokens = None;

    (url.to_string(), body)
}

/// Renders the upstream request URL and payload for an OpenAI-compatible Responses upstream: `POST
/// <base_url>/responses` (README §5.3).
pub(crate) fn render_request<'a>(
    ir: &'a IrRequest,
    base_url: &str,
    is_streaming: bool,
) -> (String, RequestBody<'a>) {
    let url = format!("{}/responses", base_url.trim_end_matches('/'));

    (url, render_request_body(ir, is_streaming))
}

/// Renders an IR request into the upstream Responses payload shared by the Codex and compatible
/// entry points.
fn render_request_body(ir: &IrRequest, is_streaming: bool) -> RequestBody<'_> {
    let mut input: Vec<InputItem<'_>> = Vec::new();

    for message in &ir.messages {
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

                input.push(InputItem::Message {
                    role: MessageRole::User,
                    content,
                });
            }

            IrRole::Assistant => {
                for part in &message.parts {
                    match part {
                        IrPart::Text { text } => {
                            input.push(InputItem::Message {
                                role: MessageRole::Assistant,
                                content: vec![MessageContentPart::OutputText { text }],
                            });
                        }

                        IrPart::ToolCall {
                            call_id,
                            name,
                            arguments,
                            thought_signature,
                        } => {
                            input.push(InputItem::FunctionCall {
                                call_id,
                                name,
                                arguments,
                                thought_signature: thought_signature.as_deref(),
                            });
                        }

                        _ => {}
                    }
                }
            }

            IrRole::Tool => {
                for part in &message.parts {
                    if let IrPart::ToolOutput { call_id, content } = part {
                        input.push(InputItem::FunctionCallOutput {
                            call_id,
                            output: content,
                        });
                    }
                }
            }
        }
    }

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

    let tool_choice = ir
        .tool_choice
        .as_ref()
        .map(|tool_choice| match tool_choice {
            IrToolChoice::Auto => ToolChoice::Mode(ToolChoiceMode::Auto),
            IrToolChoice::None => ToolChoice::Mode(ToolChoiceMode::None),
            IrToolChoice::Required => ToolChoice::Mode(ToolChoiceMode::Required),

            IrToolChoice::Function { name } => ToolChoice::Function(NamedToolChoice {
                tool_type: "function",
                name,
            }),
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

    RequestBody {
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
        reasoning: ir.reasoning_effort.map(|effort| Reasoning {
            effort: effort.to_openai(),
        }),
    }
}
