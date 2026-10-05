//! Client-request parsing for the Chat Completions codec: the outward `POST /chat/completions` body
//! deserialized into the IR request.

use crate::{
    format::{self, ParsedRequest, StringOrArray},
    ir::{
        InvalidRequest, IrImage, IrMessage, IrPart, IrReasoningLevel, IrRequest, IrResponseFormat,
        IrRole, IrTool, IrToolChoice,
    },
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
    /// and the value is carried verbatim into the upstream's own parameter (README §6.4).
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

        /// Turn name; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        name: Option<IgnoredAny>,
    },

    /// Developer instruction.
    Developer {
        /// Instruction text: a plain string or an array of text parts.
        content: StringOrArray<TextPartIn>,

        /// Turn name; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        name: Option<IgnoredAny>,
    },

    /// User turn.
    User {
        /// Text or mixed text/image content.
        content: StringOrArray<UserPartIn>,

        /// Turn name; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        name: Option<IgnoredAny>,
    },

    /// Assistant turn. The `reasoning` and `reasoning_content` fields, when present, are accepted
    /// and ignored: they carry no replayable state in a stateless gateway, and the gateway itself
    /// emits `reasoning_content`, so clients may replay it. The same disposition applies to the
    /// officially-typed `refusal`, `name`, `audio`, and `annotations` members: the reference
    /// SDKs' canonical history replay appends each response message's `model_dump()` verbatim as
    /// the next assistant turn, and that object carries them, so rejecting any of them would 400
    /// turn two of that replay (README §6.6).
    Assistant {
        /// Text content, when any.
        #[serde(default)]
        content: Option<StringOrArray<TextPartIn>>,

        /// Tool invocations made during the turn.
        #[serde(default)]
        tool_calls: Option<Vec<ToolCallIn>>,

        /// The legacy protocol's single invocation, parsed into the ordinary tool-call IR under
        /// its synthesized stable id; modern `tool_calls` take precedence when both are present
        /// (README §6.2).
        #[serde(default)]
        function_call: Option<ToolCallFunctionIn>,

        /// Reasoning text under the OpenRouter/vLLM key; accepted and ignored, because the gateway
        /// itself emits `reasoning_content` and a stateless gateway has no replayable reasoning
        /// state (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        reasoning: Option<IgnoredAny>,

        /// Reasoning text under the DeepSeek de-facto key; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        reasoning_content: Option<IgnoredAny>,

        /// Upstream refusal text; accepted and ignored: the gateway already renders upstream
        /// refusal text as ordinary assistant text (README §6.2), so a replayed turn carries it
        /// with no replayable state behind it (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        refusal: Option<IgnoredAny>,

        /// Turn name; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        name: Option<IgnoredAny>,

        /// Audio output payload; accepted and ignored: a stateless gateway carries no replayable
        /// audio state (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        audio: Option<IgnoredAny>,

        /// Citation annotations; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
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

        /// OpenAI image detail hint beside the reference; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
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
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
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
/// the same stable spelling the upstream direction mints (README §6.2).
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
                // upstream direction applies to `message.function_call` (README §6.2).
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

    format::validate_sampling_ranges(request.temperature, request.top_p)?;

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
            // (README §6.4).
            reasoning_effort: request
                .reasoning_effort
                .as_deref()
                .map(|effort| IrReasoningLevel::parse(effort, "reasoning_effort"))
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

    // The separator only ever separates text the accumulator already holds, so a leading run
    // of empty parts carries none: skip it, then join the rest with the blank-line separator.
    let combined = texts
        .iter()
        .skip_while(|text| text.is_empty())
        .copied()
        .collect::<Vec<&str>>()
        .join("\n\n");

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses one request body given as JSON.
    fn parse(body: Value) -> Result<ParsedRequest, InvalidRequest> {
        parse_request(body.to_string().as_bytes())
    }

    /// Plain-string content keeps parsing on the text-only roles: system and
    /// developer strings concatenate into instructions in encounter order,
    /// and a tool string becomes the tool output verbatim (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed request diverges from the documented IR.
    #[test]
    fn string_content_parses_on_text_only_roles() {
        #[expect(
            clippy::expect_used,
            reason = "the constructed body parses on every text-only role"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "messages": [
                {"role": "system", "content": "be terse"},
                {"role": "developer", "content": "no markdown"},
                {"role": "user", "content": "hi"},
                {"role": "tool", "tool_call_id": "call_1", "content": "{\"ok\": true}"},
            ],
        }))
        .expect("string content must parse on every text-only role");

        assert_eq!(
            parsed.ir.instructions.as_deref(),
            Some("be terse\n\nno markdown"),
        );

        #[expect(
            clippy::expect_used,
            reason = "the constructed body carries a tool message"
        )]
        let tool_message = parsed
            .ir
            .messages
            .last()
            .expect("the tool message must be present");

        assert_eq!(tool_message.role, IrRole::Tool);

        assert_eq!(
            tool_message.parts,
            vec![IrPart::ToolOutput {
                call_id: "call_1".to_string(),
                content: "{\"ok\": true}".to_string(),
                is_error: false,
            }],
        );
    }

    /// A text-parts array flattens to exactly the plain-string form's
    /// content: parts join with the blank-line separator, and the flattened
    /// system/developer texts concatenate into instructions unchanged, so
    /// both wire forms produce identical IR (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed requests diverge from the documented IR.
    #[test]
    fn text_part_arrays_flatten_to_the_string_form() {
        #[expect(
            clippy::expect_used,
            reason = "the constructed string-form body parses"
        )]
        let string_form = parse(serde_json::json!({
            "model": "m",
            "messages": [
                {"role": "system", "content": "a\n\nb"},
                {"role": "developer", "content": "c"},
                {"role": "user", "content": "hi"},
                {"role": "tool", "tool_call_id": "call_1", "content": "x\n\ny"},
            ],
        }))
        .expect("string content must parse");

        #[expect(clippy::expect_used, reason = "the constructed parts-form body parses")]
        let parts_form = parse(serde_json::json!({
            "model": "m",
            "messages": [
                {"role": "system", "content": [
                    {"type": "text", "text": "a"},
                    {"type": "text", "text": "b"},
                ]},
                {"role": "developer", "content": [{"type": "text", "text": "c"}]},
                {"role": "user", "content": "hi"},
                {"role": "tool", "tool_call_id": "call_1", "content": [
                    {"type": "text", "text": "x"},
                    {"type": "text", "text": "y"},
                ]},
            ],
        }))
        .expect("text-parts content must parse on every text-only role");

        assert_eq!(parts_form.ir.instructions, string_form.ir.instructions);

        assert_eq!(parts_form.ir.instructions.as_deref(), Some("a\n\nb\n\nc"));

        assert_eq!(parts_form.ir.messages, string_form.ir.messages);

        #[expect(
            clippy::expect_used,
            reason = "the constructed body carries a tool message"
        )]
        let tool_message = parts_form
            .ir
            .messages
            .last()
            .expect("the tool message must be present");

        assert_eq!(
            tool_message.parts,
            vec![IrPart::ToolOutput {
                call_id: "call_1".to_string(),
                content: "x\n\ny".to_string(),
                is_error: false,
            }],
        );
    }

    /// An image part carrying the official `detail` hint parses to the same
    /// IR as the part without it: typed SDK clients send `detail` on every
    /// image part, and the gateway cannot honor it downstream, so it is
    /// accepted and ignored like `strict` (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed requests diverge from the documented IR.
    #[test]
    fn image_url_detail_is_accepted_and_ignored() {
        #[expect(
            clippy::expect_used,
            reason = "the plain image body is constructed to parse"
        )]
        let plain = parse(serde_json::json!({
            "model": "m",
            "messages": [
                {"role": "user", "content": [
                    {
                        "type": "image_url",
                        "image_url": {"url": "https://example.com/i.png"},
                    },
                ]},
            ],
        }))
        .expect("a plain image part must parse");

        #[expect(
            clippy::expect_used,
            reason = "the detailed image body is constructed to parse"
        )]
        let detailed = parse(serde_json::json!({
            "model": "m",
            "messages": [
                {"role": "user", "content": [
                    {
                        "type": "image_url",
                        "image_url": {
                            "url": "https://example.com/i.png",
                            "detail": "high",
                        },
                    },
                ]},
            ],
        }))
        .expect("an image part carrying `detail` must parse");

        assert_eq!(detailed.ir.messages, plain.ir.messages);
    }

    /// Only `{"type": "text"}` parts are valid inside a text-only role's
    /// content array per the official schema; any other part type is
    /// rejected naming it (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if a rejection lands outside its documented wording.
    #[test]
    fn non_text_parts_on_text_only_roles_are_rejected() {
        for role in ["system", "developer", "tool"] {
            let content = serde_json::json!([
                {"type": "text", "text": "before"},
                {"type": "image_url", "image_url": {"url": "https://example.com/i.png"}},
            ]);

            let message = if role == "tool" {
                serde_json::json!({
                    "role": role,
                    "content": content,
                    "tool_call_id": "call_1",
                })
            } else {
                serde_json::json!({"role": role, "content": content})
            };

            #[expect(
                clippy::expect_used,
                reason = "the non-text part is constructed to be rejected"
            )]
            let error = parse(serde_json::json!({
                "model": "m",
                "messages": [message],
            }))
            .expect_err("a non-text part must be rejected on a text-only role");

            assert_eq!(error.param.as_deref(), Some("content"));

            assert_eq!(
                error.to_string(),
                format!("{role} message content parts must be text; `image_url` is not supported"),
            );
        }
    }

    /// Unknown members are rejected wherever the protocol defines a shape: a
    /// message member outside the message's own shape, a content-part member
    /// outside the part's own shape, and a member beside a fieldless response
    /// format. serde's rejection names the offending member, which is the
    /// contract README §6.6 documents.
    ///
    /// # Panics
    ///
    /// Panics if a rejection lands outside its documented wording.
    #[test]
    fn unknown_members_are_rejected() {
        let cases = [
            (
                serde_json::json!({
                    "model": "m",
                    "messages": [{"role": "user", "content": "hi", "bogus": 1}],
                }),
                "bogus",
            ),
            (
                serde_json::json!({
                    "model": "m",
                    "messages": [{
                        "role": "user",
                        "content": [{"type": "text", "text": "hi", "bogus": 1}],
                    }],
                }),
                "bogus",
            ),
            (
                serde_json::json!({
                    "model": "m",
                    "messages": [{"role": "user", "content": "hi"}],
                    "response_format": {"type": "json_object", "bogus": 1},
                }),
                "bogus",
            ),
        ];

        for (body, field) in cases {
            #[expect(
                clippy::expect_used,
                reason = "each unknown member is constructed to be rejected"
            )]
            let error = parse(body).expect_err("an unknown member must be rejected");

            assert!(
                error
                    .to_string()
                    .contains(&format!("unknown field `{field}`")),
                "unexpected rejection: {error}"
            );
        }
    }

    /// An assistant turn carrying the officially-current members the gateway cannot honor —
    /// `refusal`, `name`, `audio`, `annotations` — parses to identical IR as the turn without
    /// them: the reference SDKs' canonical history replay appends each response message's
    /// `model_dump()` verbatim as the next assistant turn, so rejecting any of these members
    /// 400s turn two of that replay (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed requests diverge from the documented IR.
    #[test]
    fn assistant_official_members_are_accepted_and_ignored() {
        #[expect(clippy::expect_used, reason = "the plain body is constructed to parse")]
        let plain = parse(serde_json::json!({
            "model": "m",
            "messages": [{"role": "assistant", "content": "hi"}],
        }))
        .expect("a plain assistant turn must parse");

        #[expect(
            clippy::expect_used,
            reason = "the decorated body is constructed to parse"
        )]
        let decorated = parse(serde_json::json!({
            "model": "m",
            "messages": [{
                "role": "assistant",
                "content": "hi",
                "name": "assistant",
                "refusal": null,
                "audio": {"id": "audio_1", "data": "b64", "transcript": "hi"},
                "annotations": [{"type": "url_citation", "url": "https://example.com"}],
            }],
        }))
        .expect("an assistant turn carrying the official members must parse");

        assert_eq!(decorated.ir.messages, plain.ir.messages);
    }

    /// `name` on the system, developer, and user messages parses to identical IR as the
    /// messages without it: the official request-param types carry the member on every one of
    /// those roles (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed requests diverge from the documented IR.
    #[test]
    fn message_name_is_accepted_and_ignored() {
        #[expect(clippy::expect_used, reason = "the plain body is constructed to parse")]
        let plain = parse(serde_json::json!({
            "model": "m",
            "messages": [
                {"role": "system", "content": "be terse"},
                {"role": "developer", "content": "no markdown"},
                {"role": "user", "content": "hi"},
            ],
        }))
        .expect("the plain body must parse");

        #[expect(clippy::expect_used, reason = "the named body is constructed to parse")]
        let named = parse(serde_json::json!({
            "model": "m",
            "messages": [
                {"role": "system", "content": "be terse", "name": "instructions"},
                {"role": "developer", "content": "no markdown", "name": "policies"},
                {"role": "user", "content": "hi", "name": "client"},
            ],
        }))
        .expect("a name on every string-carrying role must parse");

        assert_eq!(named.ir.messages, plain.ir.messages);

        assert_eq!(named.ir.instructions, plain.ir.instructions);
    }

    /// The legacy `function_call` member parses into the ordinary tool-call IR exactly as the
    /// upstream direction maps the protocol: the single call rides the assembly's default index
    /// under its synthesized stable id, modern `tool_calls` take precedence when one message
    /// carries both, and the replayed arguments must encode a JSON object like the modern
    /// calls' do (README §6.2, §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed requests or rejection diverge from the documented contract.
    #[test]
    fn legacy_function_call_parses_into_the_tool_call_ir() {
        #[expect(
            clippy::expect_used,
            reason = "the legacy body is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "messages": [{
                "role": "assistant",
                "content": null,
                "function_call": {"name": "get_weather", "arguments": "{\"city\": \"Paris\"}"},
            }],
        }))
        .expect("a legacy function_call must parse into the tool-call IR");

        #[expect(clippy::expect_used, reason = "the body carries one assistant message")]
        let message = parsed.ir.messages.last().expect("the turn must be present");

        assert_eq!(
            message.parts,
            vec![IrPart::ToolCall {
                call_id: "call_0".to_string(),
                name: "get_weather".to_string(),
                arguments: "{\"city\": \"Paris\"}".to_string(),
                thought_signature: None,
            }],
        );

        // Modern `tool_calls` take precedence over a legacy call on the same message.
        #[expect(
            clippy::expect_used,
            reason = "the both-present body is constructed to parse"
        )]
        let both = parse(serde_json::json!({
            "model": "m",
            "messages": [{
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_9",
                    "type": "function",
                    "function": {"name": "modern", "arguments": "{}"},
                }],
                "function_call": {"name": "legacy", "arguments": "{}"},
            }],
        }))
        .expect("a message carrying both call forms must parse");

        #[expect(clippy::expect_used, reason = "the body carries one assistant message")]
        let message = both.ir.messages.last().expect("the turn must be present");

        assert_eq!(
            message.parts,
            vec![IrPart::ToolCall {
                call_id: "call_9".to_string(),
                name: "modern".to_string(),
                arguments: "{}".to_string(),
                thought_signature: None,
            }],
        );

        // An empty modern array leaves the assembly to the legacy call — the upstream
        // direction's precedence rule consults the legacy member whenever no modern entry is
        // assembled (README §6.2).
        #[expect(
            clippy::expect_used,
            reason = "the empty-array body is constructed to parse"
        )]
        let empty_modern = parse(serde_json::json!({
            "model": "m",
            "messages": [{
                "role": "assistant",
                "tool_calls": [],
                "function_call": {"name": "legacy", "arguments": "{}"},
            }],
        }))
        .expect("a legacy call beside an empty modern array must parse");

        #[expect(clippy::expect_used, reason = "the body carries one assistant message")]
        let message = empty_modern
            .ir
            .messages
            .last()
            .expect("the turn must be present");

        assert_eq!(
            message.parts,
            vec![IrPart::ToolCall {
                call_id: "call_0".to_string(),
                name: "legacy".to_string(),
                arguments: "{}".to_string(),
                thought_signature: None,
            }],
        );

        // Symmetric with the modern calls: replayed arguments must encode a JSON object.
        #[expect(
            clippy::expect_used,
            reason = "the non-object arguments are constructed to be rejected"
        )]
        let error = parse(serde_json::json!({
            "model": "m",
            "messages": [{
                "role": "assistant",
                "function_call": {"name": "get_weather", "arguments": "\"Paris\""},
            }],
        }))
        .expect_err("non-object legacy arguments must be rejected");

        assert_eq!(error.param.as_deref(), Some("arguments"));
    }

    /// `reasoning_effort` parses into the closed seven-value vocabulary (README §6.4): known
    /// values parse, unknown strings are a 400 naming the field, and a non-string value is a 400
    /// malformed request.
    ///
    /// # Panics
    ///
    /// Panics if the parsed request or rejection diverges from the contract.
    #[test]
    fn reasoning_effort_parses_the_closed_vocabulary() {
        use crate::ir::IrReasoningLevel;

        for (level, effort) in IrReasoningLevel::VOCABULARY {
            #[expect(
                clippy::expect_used,
                reason = "a vocabulary string is constructed to parse"
            )]
            let parsed = parse(serde_json::json!({
                "model": "m",
                "messages": [{"role": "user", "content": "hi"}],
                "reasoning_effort": effort,
            }))
            .expect("a vocabulary effort string must parse");

            assert_eq!(parsed.ir.reasoning_effort, Some(level));
        }

        #[expect(
            clippy::expect_used,
            reason = "the unknown value is constructed to be rejected"
        )]
        let error = parse(serde_json::json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi"}],
            "reasoning_effort": "ultra",
        }))
        .expect_err("an unknown effort must be rejected");

        assert_eq!(error.param.as_deref(), Some("reasoning_effort"));

        #[expect(
            clippy::expect_used,
            reason = "the non-string value is constructed to be rejected"
        )]
        let error = parse(serde_json::json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi"}],
            "reasoning_effort": 5,
        }))
        .expect_err("a non-string effort must be rejected as malformed");

        assert!(
            error.to_string().contains("invalid type"),
            "a non-string effort fails on its type alone: {error}"
        );
    }
}
