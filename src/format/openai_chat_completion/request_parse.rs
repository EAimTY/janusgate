//! Client-request parsing for the Chat Completions codec: the outward
//! `POST /chat/completions` body deserialized into the IR request.

use crate::{
    format::{
        ParsedRequest, allowed_fields, arguments_as_object, parse_tool_choice_mode,
        reject_non_function_type, reject_tool_choice_without_tools, reject_unknown_fields,
        validate_content_part_fields, validate_sampling_ranges,
    },
    ir::{
        InvalidRequest, IrImage, IrMessage, IrPart, IrReasoningEffort, IrRequest, IrResponseFormat,
        IrRole, IrTool, IrToolChoice,
    },
};
use serde::Deserialize;
use serde_json::Value;

/// Fields allowed on each chat message object, keyed by role. Unknown fields
/// are rejected because internally tagged serde enums cannot enforce this.
const MESSAGE_FIELDS_BY_ROLE: &[(&str, &[&str])] = &[
    ("system", &["role", "content"]),
    ("developer", &["role", "content"]),
    ("user", &["role", "content"]),
    (
        "assistant",
        &[
            "role",
            "content",
            "reasoning",
            "reasoning_content",
            "tool_calls",
        ],
    ),
    ("tool", &["role", "content", "tool_call_id"]),
];

/// Fields allowed on each chat content part, keyed by part type.
const CONTENT_PART_FIELDS: &[(&str, &[&str])] = &[
    ("text", &["type", "text"]),
    ("image_url", &["type", "image_url"]),
];

/// Strictly parsed Chat Completions request body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestBody {
    /// Public model name to route to.
    model: String,

    /// Conversation messages.
    messages: Vec<MessageIn>,

    /// Function tools advertised to the model.
    #[serde(default)]
    tools: Option<Vec<ToolIn>>,

    /// Tool selection restriction.
    #[serde(default)]
    tool_choice: Option<ToolChoiceIn>,

    /// Sampling temperature.
    #[serde(default)]
    temperature: Option<f64>,

    /// Nucleus sampling threshold.
    #[serde(default)]
    top_p: Option<f64>,

    /// Legacy output token budget.
    #[serde(default)]
    max_tokens: Option<u64>,

    /// Modern output token budget.
    #[serde(default)]
    max_completion_tokens: Option<u64>,

    /// Response format restriction.
    #[serde(default)]
    response_format: Option<ResponseFormatIn>,

    /// OpenAI reasoning effort.
    #[serde(default)]
    reasoning_effort: Option<String>,

    /// Whether the client wants a streamed response.
    #[serde(default)]
    stream: Option<bool>,

    /// Stream shaping options.
    #[serde(default)]
    stream_options: Option<StreamOptionsIn>,
}

/// Chat message, tagged by `role`.
#[derive(Debug, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum MessageIn {
    /// System instruction.
    System {
        /// Instruction text.
        content: String,
    },

    /// Developer instruction.
    Developer {
        /// Instruction text.
        content: String,
    },

    /// User turn.
    User {
        /// Text or mixed text/image content.
        content: UserContentIn,
    },

    /// Assistant turn. The `reasoning` and `reasoning_content` fields, when
    /// present, are accepted and ignored: they carry no replayable state in a
    /// stateless gateway, and the gateway itself emits `reasoning_content`,
    /// so clients may replay it.
    Assistant {
        /// Text content, when any.
        #[serde(default)]
        content: Option<AssistantContentIn>,

        /// Tool invocations made during the turn.
        #[serde(default)]
        tool_calls: Option<Vec<ToolCallIn>>,
    },

    /// Tool result.
    Tool {
        /// Result content.
        content: String,

        /// Identifier of the invocation being answered.
        tool_call_id: String,
    },
}

/// User message content: plain string or typed parts.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum UserContentIn {
    /// Plain text.
    Text(String),

    /// Typed parts.
    Parts(Vec<UserPartIn>),
}

/// User content part, tagged by `type`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UserPartIn {
    /// Text segment.
    Text {
        /// The text content.
        text: String,
    },

    /// Image segment.
    ImageUrl {
        /// The image reference.
        image_url: ImageUrlIn,
    },
}

/// Image reference within a user part.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageUrlIn {
    /// Data URL (`data:<mime>;base64,<data>`) or remote HTTP(S) URL.
    url: String,
}

/// Assistant message content: plain string or text parts.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AssistantContentIn {
    /// Plain text.
    Text(String),

    /// Typed parts.
    Parts(Vec<AssistantPartIn>),
}

/// Assistant content part, tagged by `type`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AssistantPartIn {
    /// Text segment.
    Text {
        /// The text content.
        text: String,
    },
}

/// Tool invocation within an assistant message.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolCallIn {
    /// Invocation identifier.
    id: String,

    /// Tool kind; only `function` exists.
    #[serde(rename = "type")]
    tool_type: String,

    /// The invoked function.
    function: ToolCallFunctionIn,

    /// Thought signature to reattach across turns.
    #[serde(default)]
    thought_signature: Option<String>,
}

/// Function payload of a tool invocation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolCallFunctionIn {
    /// Function name.
    name: String,

    /// JSON-encoded arguments.
    #[serde(default)]
    arguments: String,
}

/// Advertised tool, tagged by `type`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolIn {
    /// Tool kind; only `function` is supported.
    #[serde(rename = "type")]
    tool_type: String,

    /// The function declaration.
    function: ToolFunctionIn,
}

/// Function tool declaration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolFunctionIn {
    /// Function name.
    name: String,

    /// Human-readable description.
    #[serde(default)]
    description: Option<String>,

    /// JSON Schema of accepted arguments.
    #[serde(default)]
    parameters: Option<Value>,

    /// OpenAI strictness flag; accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    strict: Option<bool>,
}

/// Tool choice, tagged by `type` when an object.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ToolChoiceIn {
    /// Named mode (`auto`, `none`, `required`).
    Mode(String),

    /// Forced function.
    Function {
        /// Tool kind; must be `function`.
        #[serde(rename = "type")]
        tool_type: String,

        /// The forced function.
        function: ToolChoiceFunctionIn,
    },
}

/// Forced function reference.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolChoiceFunctionIn {
    /// Required function name.
    name: String,
}

/// Response format restriction, tagged by `type`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseFormatIn {
    /// Free-form text.
    Text,

    /// Any valid JSON value.
    JsonObject,

    /// JSON conforming to a named schema.
    JsonSchema {
        /// The schema envelope.
        json_schema: JsonSchemaIn,
    },
}

/// Named JSON Schema envelope.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonSchemaIn {
    /// Schema name.
    name: String,

    /// The JSON Schema itself.
    schema: Value,

    /// OpenAI strictness flag; accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    strict: Option<bool>,
}

/// Stream shaping options.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StreamOptionsIn {
    /// Whether a final usage-only chunk is emitted.
    include_usage: bool,
}

/// Parses a Chat Completions request body into IR.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for malformed JSON, unknown or unsupported
/// fields, invalid parameter ranges, and invalid image references.
pub(super) fn parse_request(body: &[u8]) -> Result<ParsedRequest, InvalidRequest> {
    let raw: Value = serde_json::from_slice(body).map_err(InvalidRequest::from_json)?;

    // Raw-field validation runs before deserialization so `raw` can be
    // consumed directly instead of cloned; deserializing a body with inline
    // base64 images can reach the 64 MB request cap.
    validate_message_fields(raw.get("messages"))?;
    validate_response_format_fields(raw.get("response_format"))?;

    let request: RequestBody = serde_json::from_value(raw).map_err(InvalidRequest::from_json)?;

    let mut instructions_parts: Vec<String> = Vec::new();
    let mut messages: Vec<IrMessage> = Vec::new();

    for message in &request.messages {
        match message {
            MessageIn::System { content } | MessageIn::Developer { content } => {
                instructions_parts.push(content.clone());
            }

            MessageIn::User { content } => {
                let parts = match content {
                    UserContentIn::Text(text) => vec![IrPart::Text { text: text.clone() }],
                    UserContentIn::Parts(parts) => {
                        let mut converted = Vec::with_capacity(parts.len());

                        for part in parts {
                            converted.push(match part {
                                UserPartIn::Text { text } => IrPart::Text { text: text.clone() },
                                UserPartIn::ImageUrl { image_url } => IrPart::Image {
                                    image: IrImage::from_url(&image_url.url)?,
                                },
                            });
                        }

                        converted
                    }
                };

                messages.push(IrMessage {
                    role: IrRole::User,
                    parts,
                });
            }

            MessageIn::Assistant {
                content,
                tool_calls,
            } => {
                let mut parts: Vec<IrPart> = Vec::new();

                if let Some(content) = content {
                    match content {
                        AssistantContentIn::Text(text) => {
                            parts.push(IrPart::Text { text: text.clone() })
                        }
                        AssistantContentIn::Parts(parts_in) => {
                            for part in parts_in {
                                let AssistantPartIn::Text { text } = part;

                                parts.push(IrPart::Text { text: text.clone() });
                            }
                        }
                    }
                }

                if let Some(tool_calls) = tool_calls {
                    for tool_call in tool_calls {
                        if tool_call.tool_type != "function" {
                            return Err(InvalidRequest::field(
                                "type",
                                format!("Unsupported tool call type `{}`", tool_call.tool_type),
                            ));
                        }

                        // Symmetric with the Responses endpoint: replayed
                        // arguments must encode a JSON object, so malformed
                        // replays fail here instead of at upstream render
                        // time.
                        arguments_as_object(&tool_call.function.arguments)?;

                        parts.push(IrPart::ToolCall {
                            call_id: tool_call.id.clone(),
                            name: tool_call.function.name.clone(),
                            arguments: tool_call.function.arguments.clone(),
                            thought_signature: tool_call.thought_signature.clone(),
                        });
                    }
                }

                messages.push(IrMessage {
                    role: IrRole::Assistant,
                    parts,
                });
            }

            MessageIn::Tool {
                content,
                tool_call_id,
            } => {
                messages.push(IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id: tool_call_id.clone(),
                        content: content.clone(),
                    }],
                });
            }
        }
    }

    let mut tools: Vec<IrTool> = Vec::new();

    if let Some(tools_in) = &request.tools {
        for tool in tools_in {
            reject_non_function_type(&tool.tool_type, "tool")?;

            tools.push(IrTool {
                name: tool.function.name.clone(),
                description: tool.function.description.clone(),
                parameters: tool.function.parameters.clone(),
            });
        }
    }

    let tool_choice = match &request.tool_choice {
        None => None,
        Some(choice) => {
            reject_tool_choice_without_tools(tools.is_empty())?;

            Some(match choice {
                ToolChoiceIn::Mode(mode) => parse_tool_choice_mode(mode)?,

                ToolChoiceIn::Function {
                    tool_type,
                    function,
                } => {
                    reject_non_function_type(tool_type, "tool_choice")?;

                    IrToolChoice::Function {
                        name: function.name.clone(),
                    }
                }
            })
        }
    };

    validate_sampling_ranges(request.temperature, request.top_p)?;

    if request.max_tokens.is_some() && request.max_completion_tokens.is_some() {
        return Err(InvalidRequest::field(
            "max_tokens",
            "max_tokens and max_completion_tokens are mutually exclusive",
        ));
    }

    let response_format = match &request.response_format {
        None => None,
        Some(ResponseFormatIn::Text) => Some(IrResponseFormat::Text),
        Some(ResponseFormatIn::JsonObject) => Some(IrResponseFormat::Json),
        Some(ResponseFormatIn::JsonSchema { json_schema }) => Some(IrResponseFormat::JsonSchema {
            name: json_schema.name.clone(),
            schema: json_schema.schema.clone(),
        }),
    };

    let reasoning_effort = match &request.reasoning_effort {
        None => None,
        Some(effort) => Some(IrReasoningEffort::parse_openai(effort).ok_or_else(|| {
            InvalidRequest::field(
                "reasoning_effort",
                format!("Unknown reasoning_effort `{effort}`"),
            )
        })?),
    };

    Ok(ParsedRequest {
        ir: IrRequest {
            model: request.model,
            instructions: if instructions_parts.is_empty() {
                None
            } else {
                Some(instructions_parts.join("\n\n"))
            },
            messages,
            tools,
            tool_choice,
            temperature: request.temperature,
            top_p: request.top_p,
            max_output_tokens: request.max_completion_tokens.or(request.max_tokens),
            response_format,
            reasoning_effort,
        },
        is_streaming: request.stream.unwrap_or(false),
        should_include_usage: request
            .stream_options
            .as_ref()
            .is_some_and(|options| options.include_usage),
    })
}

/// Rejects unknown fields on chat message objects; internally tagged serde
/// enums cannot enforce this themselves.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming the first unknown field.
fn validate_message_fields(messages: Option<&Value>) -> Result<(), InvalidRequest> {
    let Some(messages) = messages.and_then(Value::as_array) else {
        return Ok(());
    };

    for message in messages {
        let Some(object) = message.as_object() else {
            continue;
        };

        let role = object
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let Some(allowed_fields) = allowed_fields(MESSAGE_FIELDS_BY_ROLE, role) else {
            continue;
        };

        reject_unknown_fields(object, allowed_fields, &format!("`{role}` message"))?;

        if let Some(parts) = object.get("content").and_then(Value::as_array) {
            validate_content_part_fields(parts, CONTENT_PART_FIELDS)?;
        }
    }

    Ok(())
}

/// Rejects unknown fields on the `response_format` envelope around its
/// internally tagged variants.
fn validate_response_format_fields(response_format: Option<&Value>) -> Result<(), InvalidRequest> {
    let Some(object) = response_format.and_then(Value::as_object) else {
        return Ok(());
    };

    let format_type = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let permitted_fields: &[&str] = match format_type {
        "json_schema" => &["type", "json_schema"],
        _ => &["type"],
    };

    reject_unknown_fields(object, permitted_fields, "response_format")
}
