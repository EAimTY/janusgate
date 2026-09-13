//! Client-request parsing for the Chat Completions codec: the outward
//! `POST /chat/completions` body deserialized into the IR request.

use crate::{
    format::{self, ParsedRequest},
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
    ("image_url", &["type", "image_url", "detail"]),
];

/// Roles whose array `content` may carry only `{"type": "text"}` parts,
/// matching the official wire schema for those roles.
const TEXT_ONLY_CONTENT_ROLES: &[&str] = &["system", "developer", "tool"];

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
        /// Instruction text: a plain string or an array of text parts.
        content: TextContentIn,
    },

    /// Developer instruction.
    Developer {
        /// Instruction text: a plain string or an array of text parts.
        content: TextContentIn,
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
        content: Option<TextContentIn>,

        /// Tool invocations made during the turn.
        #[serde(default)]
        tool_calls: Option<Vec<ToolCallIn>>,
    },

    /// Tool result.
    Tool {
        /// Result content: a plain string or an array of text parts.
        content: TextContentIn,

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

/// Text-only message content: a plain string or typed text parts. The
/// system, developer, assistant, and tool roles all share this wire shape;
/// only user content carries image parts.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TextContentIn {
    /// Plain text.
    Text(String),

    /// Typed parts.
    Parts(Vec<TextPartIn>),
}

/// Text content part, tagged by `type`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TextPartIn {
    /// Text segment.
    Text {
        /// The text content.
        text: String,
    },
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
                instructions_parts.push(flatten_text_content(content));
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
                        TextContentIn::Text(text) => {
                            parts.push(IrPart::Text { text: text.clone() })
                        }
                        TextContentIn::Parts(parts_in) => {
                            for part in parts_in {
                                let TextPartIn::Text { text } = part;

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
                        content: flatten_text_content(content),
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
                ToolChoiceIn::Mode(mode) => format::parse_tool_choice_mode(mode)?,

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

/// Flattens string-or-text-parts content into a single string. Parts join
/// with the same blank-line separator used to concatenate system/developer
/// messages into IR instructions, so the array form flattens to exactly what
/// the equivalent plain-string form carries.
fn flatten_text_content(content: &TextContentIn) -> String {
    match content {
        TextContentIn::Text(text) => text.clone(),

        TextContentIn::Parts(parts) => parts
            .iter()
            .map(|TextPartIn::Text { text }| text.clone())
            .collect::<Vec<_>>()
            .join("\n\n"),
    }
}

/// Rejects unknown fields on chat message objects; internally tagged serde
/// enums cannot enforce this themselves. Also rejects non-text part types in
/// the array content of the text-only roles, where the official wire schema
/// admits only `{"type": "text"}` parts.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming the first unknown field or the first
/// unsupported content part type.
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

        let Some(allowed_fields) = format::allowed_fields(MESSAGE_FIELDS_BY_ROLE, role) else {
            continue;
        };

        format::reject_unknown_fields(object, allowed_fields, &format!("`{role}` message"))?;

        if let Some(parts) = object.get("content").and_then(Value::as_array) {
            format::validate_content_part_fields(parts, CONTENT_PART_FIELDS)?;

            // A recognizable non-text part on a text-only role is rejected
            // here, naming the part type, instead of surfacing as an
            // untagged-enum serde failure; malformed parts without a `type`
            // string are left to serde's rejection.
            if TEXT_ONLY_CONTENT_ROLES.contains(&role) {
                for part in parts {
                    let Some(part_type) = part.get("type").and_then(Value::as_str) else {
                        continue;
                    };

                    if part_type != "text" {
                        return Err(InvalidRequest::field(
                            "content",
                            format!(
                                "{role} message content parts must be text; \
                                 `{part_type}` is not supported"
                            ),
                        ));
                    }
                }
            }
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

    format::reject_unknown_fields(object, permitted_fields, "response_format")
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
    #[test]
    fn string_content_parses_on_text_only_roles() {
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
            }],
        );
    }

    /// A text-parts array flattens to exactly the plain-string form's
    /// content: parts join with the blank-line separator, and the flattened
    /// system/developer texts concatenate into instructions unchanged, so
    /// both wire forms produce identical IR (README §6.6).
    #[test]
    fn text_part_arrays_flatten_to_the_string_form() {
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
            }],
        );
    }

    /// An image part carrying the official `detail` hint parses to the same
    /// IR as the part without it: typed SDK clients send `detail` on every
    /// image part, and the gateway cannot honor it downstream, so it is
    /// accepted and ignored like `strict` (README §6.6).
    #[test]
    fn image_url_detail_is_accepted_and_ignored() {
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

    /// The official `none` effort — the OpenAI vocabulary's explicit
    /// no-reasoning value — parses to the canonical `off` level, the same
    /// level `minimal` maps to, and the closed vocabulary still rejects
    /// `xhigh`: the canonical set has no target for it that would not
    /// silently downgrade an explicit client request (README §6.4).
    #[test]
    fn reasoning_effort_none_parses_to_off_and_xhigh_is_rejected() {
        let none = parse(serde_json::json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi"}],
            "reasoning_effort": "none",
        }))
        .expect("`reasoning_effort: \"none\"` must parse");

        assert_eq!(none.ir.reasoning_effort, Some(IrReasoningEffort::Off));

        let error = parse(serde_json::json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi"}],
            "reasoning_effort": "xhigh",
        }))
        .expect_err("`reasoning_effort: \"xhigh\"` must be rejected");

        assert_eq!(error.param.as_deref(), Some("reasoning_effort"));

        assert_eq!(error.to_string(), "Unknown reasoning_effort `xhigh`");
    }
}
