//! Upstream-request rendering for the Chat Completions codec: renders an IR request into the
//! `openai_chat_completion` upstream payload.
//!
//! The rendered payload is a first-party type in the upstream wire shape, so field presence per
//! request is declared by `skip_serializing_if` rather than assembled conditionally.

use crate::{
    format::openai::{ToolChoice, render_tool_choice},
    ir::{InvalidRequest, IrImage, IrPart, IrRequest, IrResponseFormat, IrRole},
};
use serde::Serialize;
use serde_json::Value;
use std::borrow::Cow;

/// The upstream Chat Completions request payload.
#[derive(Debug, Serialize)]
pub(crate) struct RequestBody<'a> {
    /// Upstream model name.
    model: &'a str,

    /// Conversation messages in order.
    messages: Vec<Message<'a>>,

    /// Whether the upstream streams its response.
    stream: bool,

    /// Streaming options, carried only for a streaming request.
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,

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

    /// Output token budget, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u64>,

    /// Response format restriction, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat<'a>>,

    /// Reasoning effort string rendered verbatim into the upstream's own parameter; omitted when
    /// the client sent no effort (`docs/reference/inference.md`).
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
}

/// Streaming options: the gateway always asks for the trailing usage chunk.
#[derive(Debug, Serialize)]
struct StreamOptions {
    /// Whether the upstream reports token accounting on the final chunk.
    include_usage: bool,
}

/// One upstream conversation message, tagged by its role.
#[derive(Debug, Serialize)]
#[serde(tag = "role", rename_all = "snake_case")]
enum Message<'a> {
    /// Concatenated system instructions.
    System {
        /// The instruction text.
        content: &'a str,
    },

    /// A user turn.
    User {
        /// The user content parts.
        content: Vec<UserContentPart<'a>>,
    },

    /// An assistant turn.
    Assistant {
        /// Joined assistant text; `null` for a turn that produced none.
        content: Option<String>,

        /// Tool calls the assistant requested, omitted when it made none.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall<'a>>,
    },

    /// One tool result.
    Tool {
        /// Identifier of the invocation being answered.
        tool_call_id: &'a str,

        /// Result content.
        content: &'a str,
    },
}

/// One user content part, borrowing the request's text and remote image URLs; only an inline
/// image owns its synthesized data URL.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UserContentPart<'a> {
    /// Plain text.
    Text {
        /// The text content.
        text: &'a str,
    },

    /// An image reference.
    ImageUrl {
        /// The image URL.
        image_url: ImageUrl<'a>,
    },
}

/// The image URL object of an image content part.
#[derive(Debug, Serialize)]
struct ImageUrl<'a> {
    /// Inline data URL or remote `http`/`https` URL (`docs/reference/inference.md`), borrowed
    /// verbatim for a remote image.
    url: Cow<'a, str>,
}

/// Renders an image into the OpenAI `image_url` string: a remote URL borrows verbatim, and only
/// inline data synthesizes its data URL — the inverse of [`IrImage::from_url`].
fn image_url(image: &IrImage) -> Cow<'_, str> {
    match image {
        IrImage::Url(url) => Cow::Borrowed(url),

        IrImage::Inline {
            mime_type,
            data_base64,
        } => Cow::Owned(format!("data:{mime_type};base64,{data_base64}")),
    }
}

/// One assistant tool call.
#[derive(Debug, Serialize)]
struct ToolCall<'a> {
    /// Identifier of the invocation, round-tripped to the matching result.
    id: &'a str,

    /// Tool kind, always `function`.
    #[serde(rename = "type")]
    tool_type: &'static str,

    /// The invocation itself.
    function: FunctionCall<'a>,

    /// Upstream thought signature that must be replayed on the next turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    thought_signature: Option<&'a str>,
}

/// The function invocation of a tool call.
#[derive(Debug, Serialize)]
struct FunctionCall<'a> {
    /// Invoked function name.
    name: &'a str,

    /// JSON-encoded arguments text.
    arguments: &'a str,
}

/// One advertised function tool.
#[derive(Debug, Serialize)]
struct Tool<'a> {
    /// Tool kind, always `function`.
    #[serde(rename = "type")]
    tool_type: &'static str,

    /// The advertised function.
    function: ToolFunction<'a>,
}

/// The advertised function of a tool.
#[derive(Debug, Serialize)]
struct ToolFunction<'a> {
    /// Function name the model calls.
    name: &'a str,

    /// Human-readable description.
    description: Option<&'a str>,

    /// JSON Schema of the accepted arguments, absent for a function that
    /// takes none.
    parameters: Option<&'a Value>,
}

/// The named-function `tool_choice` form.
#[derive(Debug, Serialize)]
struct NamedToolChoice<'a> {
    /// Tool kind, always `function`.
    #[serde(rename = "type")]
    tool_type: &'static str,

    /// The named function.
    function: NamedFunction<'a>,
}

/// The named function of a forced `tool_choice`.
#[derive(Debug, Serialize)]
struct NamedFunction<'a> {
    /// Required function name.
    name: &'a str,
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
        /// The named schema.
        json_schema: JsonSchema<'a>,
    },
}

/// The named schema of a `json_schema` response format.
#[derive(Debug, Serialize)]
struct JsonSchema<'a> {
    /// Schema name.
    name: &'a str,

    /// The JSON Schema itself, whose shape is the client's to define.
    schema: &'a Value,
}

/// Renders the upstream Chat Completions payload: the body only — the URL, the headers, and the
/// streaming decision belong to the provider definition (`docs/design/providers.md`).
pub(crate) fn render_request_body(
    ir: &IrRequest,
    is_streaming: bool,
) -> Result<RequestBody<'_>, InvalidRequest> {
    // The Chat Completions protocol has no compaction state channel: opaque
    // native state another surface produced is never silently discarded, so
    // its presence fails the render (the caller's replay is producer-bound).
    if !ir.state_items.is_empty() {
        return Err(InvalidRequest::request(
            "the request carries compaction state the Chat Completions upstream cannot replay",
        ));
    }

    let mut messages: Vec<Message<'_>> = Vec::new();

    if let Some(instructions) = &ir.instructions {
        messages.push(Message::System {
            content: instructions,
        });
    }

    for message in &ir.messages {
        match message.role {
            IrRole::User => {
                let content: Vec<UserContentPart<'_>> = message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        IrPart::Text { text } => Some(UserContentPart::Text { text }),

                        IrPart::Image { image } => Some(UserContentPart::ImageUrl {
                            image_url: ImageUrl {
                                url: image_url(image),
                            },
                        }),

                        // The IR type system routes only text and images into
                        // user content.
                        _ => None,
                    })
                    .collect();

                messages.push(Message::User { content });
            }

            IrRole::Assistant => {
                let texts: Vec<&String> = message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        IrPart::Text { text } => Some(text),
                        _ => None,
                    })
                    .collect();

                let tool_calls: Vec<ToolCall<'_>> = message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        IrPart::ToolCall {
                            call_id,
                            name,
                            arguments,
                            thought_signature,
                        } => Some(ToolCall {
                            id: call_id,
                            tool_type: "function",
                            function: FunctionCall { name, arguments },
                            thought_signature: thought_signature.as_deref(),
                        }),

                        _ => None,
                    })
                    .collect();

                // The Chat Completions wire spec types assistant `content` as `string | null`;
                // parts arrays are not universally accepted by compatible upstreams, and the
                // gateway attaches no per-part semantics to assistant text, so parts join like
                // system instructions do.
                let content = match texts.as_slice() {
                    [] => None,
                    texts => Some(
                        texts
                            .iter()
                            .map(|text| text.as_str())
                            .collect::<Vec<_>>()
                            .join("\n\n"),
                    ),
                };

                messages.push(Message::Assistant {
                    content,
                    tool_calls,
                });
            }

            IrRole::Tool => {
                for part in &message.parts {
                    if let IrPart::ToolOutput {
                        call_id, content, ..
                    } = part
                    {
                        messages.push(Message::Tool {
                            tool_call_id: call_id,
                            content,
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
            function: ToolFunction {
                name: &tool.name,
                description: tool.description.as_deref(),
                parameters: tool.parameters.as_ref(),
            },
        })
        .collect();

    let tool_choice = ir.tool_choice.as_ref().map(|tool_choice| {
        render_tool_choice(tool_choice, |name| NamedToolChoice {
            tool_type: "function",
            function: NamedFunction { name },
        })
    });

    let response_format =
        ir.response_format
            .as_ref()
            .map(|response_format| match response_format {
                IrResponseFormat::Text => ResponseFormat::Text,
                IrResponseFormat::Json => ResponseFormat::JsonObject,
                IrResponseFormat::JsonSchema { name, schema } => ResponseFormat::JsonSchema {
                    json_schema: JsonSchema { name, schema },
                },
            });

    Ok(RequestBody {
        model: &ir.model,
        messages,
        stream: is_streaming,
        stream_options: is_streaming.then_some(StreamOptions {
            include_usage: true,
        }),
        tools,
        tool_choice,
        temperature: ir.temperature,
        top_p: ir.top_p,
        max_completion_tokens: ir.max_output_tokens,
        response_format,
        reasoning_effort: ir.reasoning_effort.as_ref().map(|level| level.as_str()),
    })
}
