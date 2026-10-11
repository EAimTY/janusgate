//! Client-request parsing for the Chat Completions codec: the outward `POST /chat/completions` body
//! deserialized into the IR request.

use crate::{
    format::{self, ParsedRequest, StringOrArray},
    ir::{
        InvalidRequest, IrImage, IrMessage, IrPart, IrRequest, IrResponseFormat, IrRole, IrTool,
        IrToolChoice,
    },
    vocabulary::ReasoningLevel,
};
use serde::{Deserialize, de::IgnoredAny};
use serde_json::Value;

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

    /// Reasoning effort string, opaque to the gateway: the string type is the whole constraint,
    /// and the value is carried verbatim into the upstream's own parameter
    /// (`docs/reference/inference.md`).
    #[serde(default)]
    reasoning_effort: Option<String>,

    /// Whether the client wants a streamed response.
    #[serde(default)]
    stream: Option<bool>,

    /// Stream shaping options.
    #[serde(default)]
    stream_options: Option<StreamOptionsIn>,
}

/// Chat message, tagged by `role`; a member outside the message's own shape is rejected.
#[derive(Debug, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase", deny_unknown_fields)]
enum MessageIn {
    /// System instruction.
    System {
        /// Instruction text: a plain string or an array of text parts.
        content: StringOrArray<TextPartIn>,

        /// Turn name; accepted and ignored (`docs/reference/inference.md`).
        #[serde(default)]
        #[expect(
            dead_code,
            reason = "accepted and ignored per `docs/reference/inference.md`"
        )]
        name: Option<IgnoredAny>,
    },

    /// Developer instruction.
    Developer {
        /// Instruction text: a plain string or an array of text parts.
        content: StringOrArray<TextPartIn>,

        /// Turn name; accepted and ignored (`docs/reference/inference.md`).
        #[serde(default)]
        #[expect(
            dead_code,
            reason = "accepted and ignored per `docs/reference/inference.md`"
        )]
        name: Option<IgnoredAny>,
    },

    /// User turn.
    User {
        /// Text or mixed text/image content.
        content: StringOrArray<UserPartIn>,

        /// Turn name; accepted and ignored (`docs/reference/inference.md`).
        #[serde(default)]
        #[expect(
            dead_code,
            reason = "accepted and ignored per `docs/reference/inference.md`"
        )]
        name: Option<IgnoredAny>,
    },

    /// Assistant turn. The `reasoning` and `reasoning_content` fields, when present, are accepted
    /// and ignored: they carry no replayable state in a stateless gateway, and the gateway itself
    /// emits `reasoning_content`, so clients may replay it. The same disposition applies to the
    /// officially-typed `refusal`, `name`, `audio`, and `annotations` members: the reference
    /// SDKs' canonical history replay appends each response message's `model_dump()` verbatim as
    /// the next assistant turn, and that object carries them, so rejecting any of them would 400
    /// turn two of that replay (`docs/reference/inference.md`).
    Assistant {
        /// Text content, when any.
        #[serde(default)]
        content: Option<StringOrArray<TextPartIn>>,

        /// Tool invocations made during the turn.
        #[serde(default)]
        tool_calls: Option<Vec<ToolCallIn>>,

        /// The legacy protocol's single invocation, parsed into the ordinary tool-call IR under
        /// its synthesized stable id; modern `tool_calls` take precedence when both are present
        /// (`docs/reference/inference.md`).
        #[serde(default)]
        function_call: Option<ToolCallFunctionIn>,

        /// Reasoning text under the OpenRouter/vLLM key; accepted and ignored, because the gateway
        /// itself emits `reasoning_content` and a stateless gateway has no replayable reasoning
        /// state (`docs/reference/inference.md`).
        #[serde(default)]
        #[expect(
            dead_code,
            reason = "accepted and ignored per `docs/reference/inference.md`"
        )]
        reasoning: Option<IgnoredAny>,

        /// Reasoning text under the DeepSeek de-facto key; accepted and ignored
        /// (`docs/reference/inference.md`).
        #[serde(default)]
        #[expect(
            dead_code,
            reason = "accepted and ignored per `docs/reference/inference.md`"
        )]
        reasoning_content: Option<IgnoredAny>,

        /// Upstream refusal text; accepted and ignored: the gateway already renders upstream
        /// refusal text as ordinary assistant text (`docs/reference/inference.md`), so a replayed
        /// turn carries it with no replayable state behind it
        /// (`docs/reference/inference.md`).
        #[serde(default)]
        #[expect(
            dead_code,
            reason = "accepted and ignored per `docs/reference/inference.md`"
        )]
        refusal: Option<IgnoredAny>,

        /// Turn name; accepted and ignored (`docs/reference/inference.md`).
        #[serde(default)]
        #[expect(
            dead_code,
            reason = "accepted and ignored per `docs/reference/inference.md`"
        )]
        name: Option<IgnoredAny>,

        /// Audio output payload; accepted and ignored: a stateless gateway carries no replayable
        /// audio state (`docs/reference/inference.md`).
        #[serde(default)]
        #[expect(
            dead_code,
            reason = "accepted and ignored per `docs/reference/inference.md`"
        )]
        audio: Option<IgnoredAny>,

        /// Citation annotations; accepted and ignored (`docs/reference/inference.md`).
        #[serde(default)]
        #[expect(
            dead_code,
            reason = "accepted and ignored per `docs/reference/inference.md`"
        )]
        annotations: Option<IgnoredAny>,
    },

    /// Tool result.
    Tool {
        /// Result content: a plain string or an array of text parts.
        content: StringOrArray<TextPartIn>,

        /// Identifier of the invocation being answered.
        tool_call_id: String,
    },
}

/// Content part of a text-only role's array content, tagged by `type`.
///
/// The official wire schema admits only text parts on those roles. The image kind is modelled so
/// the rejection can name the offending kind, and every further kind lands in
/// [`TextPartIn::Other`], which the conversion rejects the same way.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum TextPartIn {
    /// Text segment.
    Text {
        /// The text content.
        text: String,
    },

    /// Image segment, which a text-only role does not admit.
    ImageUrl {
        /// The image reference.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        image_url: Option<IgnoredAny>,
    },

    /// Any other part kind, of which the tag value the enum machinery consumed leaves no trace.
    #[serde(other)]
    Other,
}

/// User content part, tagged by `type`; a member outside the part's own shape is rejected.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
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

        /// OpenAI image detail hint beside the reference; accepted and ignored
        /// (`docs/reference/inference.md`).
        #[serde(default)]
        #[expect(
            dead_code,
            reason = "accepted and ignored per `docs/reference/inference.md`"
        )]
        detail: Option<IgnoredAny>,
    },
}

/// Image reference within a user part.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageUrlIn {
    /// Data URL (`data:<mime>;base64,<data>`) or remote HTTP(S) URL.
    url: String,

    /// OpenAI image detail hint; accepted and ignored.
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
    detail: Option<String>,
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
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
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

/// Response format restriction, tagged by `type`; a member outside the format's own shape is
/// rejected, and the fieldless formats are empty struct variants rather than unit variants because
/// only struct variants reject unknown members.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ResponseFormatIn {
    /// Free-form text.
    Text {},

    /// Any valid JSON value.
    JsonObject {},

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
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
    strict: Option<bool>,
}

/// Stream shaping options.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StreamOptionsIn {
    /// Whether a final usage-only chunk is emitted.
    include_usage: bool,
}

impl TextPartIn {
    /// The part's wire type name, for the rejection that names the offending kind; [`None`] for a
    /// kind [`TextPartIn::Other`] swallowed.
    fn type_name(&self) -> Option<&'static str> {
        match self {
            Self::Text { .. } => Some("text"),

            Self::ImageUrl { .. } => Some("image_url"),

            Self::Other => None,
        }
    }
}

/// Call identifier synthesized for the legacy `function_call` protocol: the protocol's single
/// call occupies the tool-call assembly's default index, and the identifier names that index —
/// the same stable spelling the upstream direction mints (`docs/reference/inference.md`).
const LEGACY_FUNCTION_CALL_ID: &str = "call_0";

/// Parses a Chat Completions request body into IR.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for malformed JSON, unknown or unsupported fields, invalid parameter
/// ranges, and invalid image references.
pub(super) fn parse_request(body: &[u8]) -> Result<ParsedRequest, InvalidRequest> {
    let request: RequestBody = format::parse_client_request(body)?;

    let mut affinity =
        crate::format::affinity::Sources::new(crate::format::UpstreamFormat::OpenAiChatCompletion);
    let mut instructions_parts: Vec<String> = Vec::new();
    let mut messages: Vec<IrMessage> = Vec::new();

    for message in &request.messages {
        match message {
            MessageIn::System { content, .. } => {
                instructions_parts.push(flatten_text_content(content, "system")?);
            }

            MessageIn::Developer { content, .. } => {
                instructions_parts.push(flatten_text_content(content, "developer")?);
            }

            MessageIn::User { content, .. } => {
                // Only user content carries image parts, so no text-only rejection applies here.
                let parts = match content {
                    StringOrArray::Text(text) => {
                        vec![IrPart::Text { text: text.clone() }]
                    }

                    StringOrArray::Array(parts) => {
                        let mut converted = Vec::with_capacity(parts.len());

                        for part in parts {
                            converted.push(match part {
                                UserPartIn::Text { text } => IrPart::Text { text: text.clone() },
                                UserPartIn::ImageUrl { image_url, .. } => IrPart::Image {
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
                function_call,
                ..
            } => {
                let mut parts: Vec<IrPart> = Vec::new();

                if let Some(content) = content {
                    match content {
                        StringOrArray::Text(text) => {
                            parts.push(IrPart::Text { text: text.clone() })
                        }

                        StringOrArray::Array(parts_in) => {
                            for part in parts_in {
                                let TextPartIn::Text { text } = part else {
                                    return Err(text_only_content_rejection(
                                        "assistant",
                                        part.type_name(),
                                    ));
                                };

                                parts.push(IrPart::Text { text: text.clone() });
                            }
                        }
                    }
                }

                if let Some(tool_calls) = tool_calls {
                    for tool_call in tool_calls {
                        format::reject_non_function_type(&tool_call.tool_type, "tool")?;

                        // Symmetric with the Responses endpoint: replayed
                        // arguments must encode a JSON object, so malformed
                        // replays fail here instead of at upstream render
                        // time.
                        format::arguments_as_object(&tool_call.function.arguments)?;

                        affinity.call(true);
                        parts.push(IrPart::ToolCall {
                            call_id: tool_call.id.clone(),
                            name: tool_call.function.name.clone(),
                            arguments: tool_call.function.arguments.clone(),
                            thought_signature: tool_call.thought_signature.clone(),
                        });
                    }
                }

                // A legacy `function_call` is consulted only when the message carries no modern
                // entry — modern `tool_calls` take precedence, the same assembly rule the
                // upstream direction applies to `message.function_call`
                // (`docs/reference/inference.md`).
                if tool_calls.as_deref().is_none_or(|calls| calls.is_empty())
                    && let Some(function_call) = function_call
                {
                    // Symmetric with the modern calls above: replayed arguments must encode a
                    // JSON object, so malformed replays fail here instead of at upstream render
                    // time.
                    format::arguments_as_object(&function_call.arguments)?;

                    affinity.call(false);
                    parts.push(IrPart::ToolCall {
                        call_id: LEGACY_FUNCTION_CALL_ID.to_string(),
                        name: function_call.name.clone(),
                        arguments: function_call.arguments.clone(),
                        thought_signature: None,
                    });
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
                affinity.result(true, crate::format::affinity::ResultShape::Text);
                messages.push(IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id: tool_call_id.clone(),
                        content: flatten_text_content(content, "tool")?,
                        is_error: false,
                    }],
                });
            }
        }
    }

    let mut tools: Vec<IrTool> = Vec::new();

    if let Some(tools_in) = &request.tools {
        for tool in tools_in {
            format::reject_non_function_type(&tool.tool_type, "tool")?;

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
            format::reject_tool_choice_without_tools(tools.is_empty())?;

            Some(match choice {
                ToolChoiceIn::Mode(mode) => format::openai::parse_tool_choice_mode(mode)?,

                ToolChoiceIn::Function {
                    tool_type,
                    function,
                } => {
                    format::reject_non_function_type(tool_type, "tool_choice")?;

                    IrToolChoice::Function {
                        name: function.name.clone(),
                    }
                }
            })
        }
    };

    format::validate_sampling_ranges(request.temperature, request.top_p, "top_p")?;

    if request.max_tokens.is_some() && request.max_completion_tokens.is_some() {
        return Err(InvalidRequest::field(
            "max_tokens",
            "max_tokens and max_completion_tokens are mutually exclusive",
        ));
    }

    let response_format = match &request.response_format {
        None => None,
        Some(ResponseFormatIn::Text {}) => Some(IrResponseFormat::Text),
        Some(ResponseFormatIn::JsonObject {}) => Some(IrResponseFormat::Json),
        Some(ResponseFormatIn::JsonSchema { json_schema }) => Some(IrResponseFormat::JsonSchema {
            name: json_schema.name.clone(),
            schema: json_schema.schema.clone(),
        }),
    };

    Ok(ParsedRequest {
        affinity,
        ir: IrRequest {
            has_compaction_trigger: false,
            state_items: Vec::new(),
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
            // The closed vocabulary is enforced here: an unknown string is a 400 naming the
            // field, and the parsed enum renders verbatim into the upstream's own parameter
            // (`docs/reference/inference.md`).
            reasoning_effort: request
                .reasoning_effort
                .as_deref()
                .map(|effort| ReasoningLevel::parse(effort, "reasoning_effort"))
                .transpose()?,
        },
        is_streaming: request.stream.unwrap_or(false),
        should_include_usage: request
            .stream_options
            .as_ref()
            .is_some_and(|options| options.include_usage),
        compaction: None,
    })
}

/// Flattens string-or-text-parts content into a single string. Parts join with the same blank-line
/// separator used to concatenate system/developer messages into IR instructions, so the array form
/// flattens to exactly what the equivalent plain-string form carries.
fn flatten_text_content(
    content: &StringOrArray<TextPartIn>,
    role: &str,
) -> Result<String, InvalidRequest> {
    let parts = match content {
        StringOrArray::Text(text) => return Ok(text.clone()),

        StringOrArray::Array(parts) => parts,
    };

    let texts = parts
        .iter()
        .map(|part| match part {
            TextPartIn::Text { text } => Ok(text.as_str()),

            other => Err(text_only_content_rejection(role, other.type_name())),
        })
        .collect::<Result<Vec<_>, _>>()?;

    let combined = format::join_text_parts(texts);

    Ok(combined)
}

/// Rejects a content part on a role whose array content admits only text parts, naming the
/// offending kind when it is one the codec recognizes.
fn text_only_content_rejection(role: &str, part_type: Option<&str>) -> InvalidRequest {
    let message = match part_type {
        Some(part_type) => {
            format!("{role} message content parts must be text; `{part_type}` is not supported")
        }

        None => format!("{role} message content parts must be text"),
    };

    InvalidRequest::field("content", message)
}
