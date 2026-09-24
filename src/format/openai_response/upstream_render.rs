//! Upstream-request rendering for the Responses codec: renders an IR request into the
//! `openai_response` upstream payload.
//!
//! The rendered payload is a first-party type in the upstream wire shape, so field presence per
//! request is declared by `skip_serializing_if` rather than assembled conditionally.

use crate::{
    ir::{IrPart, IrRequest, IrResponseFormat, IrRole, IrToolChoice},
    provider::definition::inference::RenderOption,
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
    reasoning: Option<Reasoning<'a>>,
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
struct Reasoning<'a> {
    /// Reasoning effort string rendered verbatim from the IR, opaque to the gateway (README §6.4).
    effort: &'a str,
}

/// Renders the upstream Responses payload: the body only — the URL, the headers, and the
/// streaming decision belong to the provider definition (README §5.2).
///
/// The `omit_max_output_tokens` render option drops the client-requested cap: the upstream
/// rejects the parameter as unsupported, and the model's own output bound applies instead.
pub(crate) fn render_request_body<'a>(
    ir: &'a IrRequest,
    is_streaming: bool,
    render_options: &[RenderOption],
) -> RequestBody<'a> {
    let mut body = render_body(ir, is_streaming);

    if render_options.contains(&RenderOption::OmitMaxOutputTokens) {
        body.max_output_tokens = None;
    }

    body
}

/// Renders an IR request into the plain upstream Responses payload.
fn render_body(ir: &IrRequest, is_streaming: bool) -> RequestBody<'_> {
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
                    if let IrPart::ToolOutput {
                        call_id, content, ..
                    } = part
                    {
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
        reasoning: ir.reasoning_effort.as_ref().map(|level| Reasoning {
            effort: level.as_str(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::render_request_body;
    use crate::ir::{IrReasoningLevel, IrRequest};
    use serde_json::Value;

    /// Renders one upstream body for the given effort level.
    ///
    /// # Panics
    ///
    /// Panics if the rendered body fails to serialize.
    fn render_body(reasoning_effort: Option<IrReasoningLevel>) -> Value {
        let ir = IrRequest {
            model: "m".to_string(),
            reasoning_effort,
            ..IrRequest::default()
        };

        let body = render_request_body(&ir, false, &[]);

        #[expect(
            clippy::expect_used,
            reason = "the rendered payload is under test; a failing render is the failure"
        )]
        serde_json::to_value(body).expect("the body serializes")
    }

    /// Verifies the Responses effort slot of the closed-vocabulary contract (README §6.4): every
    /// vocabulary value renders verbatim, and an absent effort renders no reasoning object at all.
    ///
    /// # Panics
    ///
    /// Panics if a rendered slot diverges from the contract.
    #[test]
    fn renders_the_effort_level_verbatim_or_not_at_all() {
        for (level, effort) in [
            (IrReasoningLevel::None, "none"),
            (IrReasoningLevel::Minimal, "minimal"),
            (IrReasoningLevel::Low, "low"),
            (IrReasoningLevel::Medium, "medium"),
            (IrReasoningLevel::High, "high"),
            (IrReasoningLevel::XHigh, "xhigh"),
            (IrReasoningLevel::Max, "max"),
        ] {
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
}
