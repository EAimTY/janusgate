//! OpenAI Chat Completions protocol codec.
//!
//! Serves the outward `POST /chat/completions` endpoint and speaks the
//! `openai_chat_completion` upstream protocol.

use crate::{
    error::{ApiError, invalid_request_from_json},
    format::{finish_reason_from_openai, finish_reason_to_openai, json_insert},
    ids::new_uuid,
    ir::{
        Aggregation, FinishReason, IrEvent, IrImage, IrMessage, IrPart, IrRequest,
        IrResponseFormat, IrRole, IrTool, ReasoningEffort, ToolChoice,
    },
    sse::{SseFrame, format_data_frame},
    unix_time,
};
use serde::Deserialize;
use serde_json::Value;

/// Fields allowed on each chat message object, keyed by role. Unknown fields
/// are rejected because internally tagged serde enums cannot enforce this.
const MESSAGE_FIELDS_BY_ROLE: &[(&str, &[&str])] = &[
    ("system", &["role", "content"]),
    ("developer", &["role", "content"]),
    ("user", &["role", "content"]),
    ("assistant", &["role", "content", "reasoning", "tool_calls"]),
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

    /// Assistant turn. A `reasoning` field, when present, is accepted and
    /// ignored: it carries no replayable state in a stateless gateway.
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

/// Client request parsed into IR with endpoint options.
#[derive(Debug)]
pub(crate) struct ParsedRequest {
    /// Format-independent request.
    pub(crate) ir: IrRequest,

    /// Whether the client requested streaming.
    pub(crate) stream: bool,

    /// Whether the client requested a final usage chunk while streaming.
    pub(crate) include_usage: bool,
}

/// Parses a Chat Completions request body into IR.
///
/// # Errors
///
/// Returns [`ApiError`] for malformed JSON, unknown or unsupported fields,
/// invalid parameter ranges, and invalid image references.
pub(crate) fn parse_request(body: &[u8]) -> Result<ParsedRequest, ApiError> {
    let raw: Value = serde_json::from_slice(body).map_err(invalid_request_from_json)?;
    let request: RequestBody =
        serde_json::from_value(raw.clone()).map_err(invalid_request_from_json)?;

    validate_message_fields(raw.get("messages"))?;
    validate_response_format_fields(raw.get("response_format"))?;

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
                                    image: parse_image_url(&image_url.url)?,
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
                            return Err(ApiError::invalid_parameter(
                                "type",
                                format!("Unsupported tool call type `{}`", tool_call.tool_type),
                            ));
                        }

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
            if tool.tool_type != "function" {
                return Err(ApiError::invalid_parameter(
                    "type",
                    format!("Unsupported tool type `{}`", tool.tool_type),
                ));
            }

            tools.push(IrTool {
                name: tool.function.name.clone(),
                description: tool.function.description.clone(),
                parameters: tool.function.parameters.clone(),
            });
        }
    }

    let tool_choice = match (&request.tool_choice, tools.is_empty()) {
        (None, _) => None,
        (Some(_), true) => {
            return Err(ApiError::invalid_parameter(
                "tool_choice",
                "tool_choice requires at least one tool",
            ));
        }

        (Some(choice), false) => Some(match choice {
            ToolChoiceIn::Mode(mode) => match mode.as_str() {
                "auto" => ToolChoice::Auto,
                "none" => ToolChoice::None,
                "required" => ToolChoice::Required,
                other => {
                    return Err(ApiError::invalid_parameter(
                        "tool_choice",
                        format!("Unknown tool_choice `{other}`"),
                    ));
                }
            },

            ToolChoiceIn::Function {
                tool_type,
                function,
            } => {
                if tool_type != "function" {
                    return Err(ApiError::invalid_parameter(
                        "type",
                        format!("Unsupported tool_choice type `{tool_type}`"),
                    ));
                }

                ToolChoice::Function {
                    name: function.name.clone(),
                }
            }
        }),
    };

    if request
        .temperature
        .is_some_and(|temperature| !(0.0..=2.0).contains(&temperature))
    {
        return Err(ApiError::invalid_parameter(
            "temperature",
            "temperature must be between 0 and 2",
        ));
    }

    if request
        .top_p
        .is_some_and(|top_p| !(0.0..=1.0).contains(&top_p))
    {
        return Err(ApiError::invalid_parameter(
            "top_p",
            "top_p must be between 0 and 1",
        ));
    }

    if request.max_tokens.is_some() && request.max_completion_tokens.is_some() {
        return Err(ApiError::invalid_parameter(
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
        Some(effort) => Some(ReasoningEffort::parse_openai(effort).ok_or_else(|| {
            ApiError::invalid_parameter(
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
        stream: request.stream.unwrap_or(false),
        include_usage: request
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
/// Returns [`ApiError`] naming the first unknown field.
fn validate_message_fields(messages: Option<&Value>) -> Result<(), ApiError> {
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

        let Some((_, allowed_fields)) = MESSAGE_FIELDS_BY_ROLE
            .iter()
            .find(|(name, _)| *name == role)
        else {
            continue;
        };

        for field in object.keys() {
            if !allowed_fields.contains(&field.as_str()) {
                return Err(ApiError::invalid_parameter(
                    field,
                    format!("Unsupported field `{field}` in `{role}` message"),
                ));
            }
        }

        if let Some(parts) = object.get("content").and_then(Value::as_array) {
            validate_content_part_fields(parts)?;
        }
    }

    Ok(())
}

/// Rejects unknown fields on content part objects.
fn validate_content_part_fields(parts: &[Value]) -> Result<(), ApiError> {
    for part in parts {
        let Some(object) = part.as_object() else {
            continue;
        };

        let part_type = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let Some((_, allowed_fields)) = CONTENT_PART_FIELDS
            .iter()
            .find(|(name, _)| *name == part_type)
        else {
            continue;
        };

        for field in object.keys() {
            if !allowed_fields.contains(&field.as_str()) {
                return Err(ApiError::invalid_parameter(
                    field,
                    format!("Unsupported field `{field}` in `{part_type}` content part"),
                ));
            }
        }
    }

    Ok(())
}

/// Rejects unknown fields on the `response_format` envelope around its
/// internally tagged variants.
fn validate_response_format_fields(response_format: Option<&Value>) -> Result<(), ApiError> {
    let Some(object) = response_format.and_then(Value::as_object) else {
        return Ok(());
    };

    let format_type = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let allowed_fields: &[&str] = match format_type {
        "json_schema" => &["type", "json_schema"],
        _ => &["type"],
    };

    for field in object.keys() {
        if !allowed_fields.contains(&field.as_str()) {
            return Err(ApiError::invalid_parameter(
                field,
                format!("Unsupported field `{field}` in response_format"),
            ));
        }
    }

    Ok(())
}

/// Parses an image reference into an [`IrImage`].
///
/// # Errors
///
/// Returns [`ApiError`] for schemes other than `data:`, `http:`, and `https:`,
/// and for malformed data URLs.
fn parse_image_url(url: &str) -> Result<IrImage, ApiError> {
    if let Some(rest) = url.strip_prefix("data:") {
        let (metadata, data) = rest.split_once(',').ok_or_else(|| {
            ApiError::invalid_parameter("image_url", "Data URL is missing its payload")
        })?;

        let mime_type = metadata.strip_suffix(";base64").ok_or_else(|| {
            ApiError::invalid_parameter("image_url", "Only base64-encoded data URLs are supported")
        })?;

        return Ok(IrImage::Inline {
            mime_type: mime_type.to_string(),
            data_base64: data.to_string(),
        });
    }

    if url.starts_with("http://") || url.starts_with("https://") {
        return Ok(IrImage::Url(url.to_string()));
    }

    Err(ApiError::invalid_parameter(
        "image_url",
        "Image URLs must use the data:, http:, or https: schemes",
    ))
}

/// Renders an [`IrImage`] back into an OpenAI image URL value.
fn render_image_url(image: &IrImage) -> String {
    match image {
        IrImage::Inline {
            mime_type,
            data_base64,
        } => format!("data:{mime_type};base64,{data_base64}"),

        IrImage::Url(url) => url.clone(),
    }
}

/// Renders an IR request into an upstream Chat Completions payload.
pub(crate) fn render_request(ir: &IrRequest, stream: bool) -> Value {
    let mut messages: Vec<Value> = Vec::new();

    if let Some(instructions) = &ir.instructions {
        messages.push(serde_json::json!({"role": "system", "content": instructions}));
    }

    for message in &ir.messages {
        match message.role {
            IrRole::User => {
                let content: Vec<Value> = message
                    .parts
                    .iter()
                    .map(|part| match part {
                        IrPart::Text { text } => {
                            serde_json::json!({"type": "text", "text": text})
                        }

                        IrPart::Image { image } => {
                            let url = render_image_url(image);

                            serde_json::json!({"type": "image_url", "image_url": {"url": url}})
                        }

                        _ => Value::Null,
                    })
                    .collect();

                messages.push(serde_json::json!({"role": "user", "content": content}));
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

                let tool_calls: Vec<Value> = message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        IrPart::ToolCall {
                            call_id,
                            name,
                            arguments,
                            thought_signature,
                        } => {
                            let mut tool_call = serde_json::json!({
                                "id": call_id,
                                "type": "function",
                                "function": {"name": name, "arguments": arguments},
                            });

                            if let Some(signature) = thought_signature {
                                json_insert(
                                    &mut tool_call,
                                    "thought_signature",
                                    Value::String(signature.clone()),
                                );
                            }

                            Some(tool_call)
                        }

                        _ => None,
                    })
                    .collect();

                let content = match texts.as_slice() {
                    [] => Value::Null,
                    [only] => serde_json::json!({ "text": only }),
                    many => Value::Array(
                        many.iter()
                            .map(|text| serde_json::json!({"type": "text", "text": text}))
                            .collect(),
                    ),
                };

                let mut message_json = serde_json::json!({"role": "assistant", "content": content});

                if !tool_calls.is_empty() {
                    json_insert(&mut message_json, "tool_calls", Value::Array(tool_calls));
                }

                messages.push(message_json);
            }

            IrRole::Tool => {
                for part in &message.parts {
                    if let IrPart::ToolOutput { call_id, content } = part {
                        messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": content,
                        }));
                    }
                }
            }
        }
    }

    let mut body = serde_json::json!({
        "model": ir.model,
        "messages": messages,
        "stream": stream,
    });

    if stream {
        json_insert(
            &mut body,
            "stream_options",
            serde_json::json!({"include_usage": true}),
        );
    }

    if !ir.tools.is_empty() {
        let tools: Vec<Value> = ir
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    },
                })
            })
            .collect();

        json_insert(&mut body, "tools", Value::Array(tools));
    }

    if let Some(tool_choice) = &ir.tool_choice {
        let choice = match tool_choice {
            ToolChoice::Auto => Value::String("auto".to_string()),
            ToolChoice::None => Value::String("none".to_string()),
            ToolChoice::Required => Value::String("required".to_string()),
            ToolChoice::Function { name } => {
                serde_json::json!({"type": "function", "function": {"name": name}})
            }
        };

        json_insert(&mut body, "tool_choice", choice);
    }

    if let Some(temperature) = ir.temperature {
        json_insert(&mut body, "temperature", Value::from(temperature));
    }

    if let Some(top_p) = ir.top_p {
        json_insert(&mut body, "top_p", Value::from(top_p));
    }

    if let Some(max_output_tokens) = ir.max_output_tokens {
        json_insert(
            &mut body,
            "max_completion_tokens",
            Value::from(max_output_tokens),
        );
    }

    if let Some(response_format) = &ir.response_format {
        let rendered_format = match response_format {
            IrResponseFormat::Text => serde_json::json!({"type": "text"}),
            IrResponseFormat::Json => serde_json::json!({"type": "json_object"}),
            IrResponseFormat::JsonSchema { name, schema } => {
                let json_schema = serde_json::json!({"name": name, "schema": schema});

                serde_json::json!({"type": "json_schema", "json_schema": json_schema})
            }
        };

        json_insert(&mut body, "response_format", rendered_format);
    }

    if let Some(effort) = ir.reasoning_effort {
        json_insert(
            &mut body,
            "reasoning_effort",
            Value::String(effort.to_openai().to_string()),
        );
    }

    body
}

/// Tool call being assembled from upstream deltas.
#[derive(Debug)]
struct PendingToolCall {
    /// Stable index of the call within the response.
    index: usize,

    /// Invocation identifier.
    call_id: String,

    /// Function name.
    name: String,

    /// Arguments accumulated so far.
    arguments: String,
}

/// Parses upstream Chat Completions responses — streamed chunks or a full
/// JSON body — into IR events.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Tool calls being assembled from deltas.
    pending_tool_calls: Vec<PendingToolCall>,

    /// Whether completed calls were already flushed.
    has_flushed_calls: bool,
}

impl UpstreamParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ingests one upstream SSE frame.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for malformed JSON and upstream error payloads.
    pub(crate) fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, ApiError> {
        if frame.data.trim() == "[DONE]" {
            return Ok(self.flush_pending_calls());
        }

        let value: Value = serde_json::from_str(&frame.data)
            .map_err(|_| ApiError::upstream_unavailable("malformed JSON in upstream SSE frame"))?;

        self.ingest_chunk(value)
    }

    /// Parses a complete upstream non-streaming JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for upstream error payloads and malformed bodies.
    pub(crate) fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, ApiError> {
        reject_upstream_error(&value)?;

        let mut events = self.message_start_events(&value);

        if let Some(message) = first_choice(&value).and_then(|choice| choice.get("message")) {
            events.append(&mut message_events_from_full(message));
        }

        let finish_reason = first_choice(&value)
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(Value::as_str)
            .map(finish_reason_from_openai)
            .unwrap_or(FinishReason::Stop);

        events.push(IrEvent::Finish(finish_reason));

        if let Some(usage) = parse_usage(value.get("usage")) {
            events.push(usage);
        }

        Ok(events)
    }

    /// Ingests one streaming chunk object.
    fn ingest_chunk(&mut self, value: Value) -> Result<Vec<IrEvent>, ApiError> {
        reject_upstream_error(&value)?;

        let mut events = self.message_start_events(&value);

        if let Some(choice) = first_choice(&value) {
            if let Some(delta) = choice.get("delta") {
                if let Some(reasoning) = delta.get("reasoning").and_then(Value::as_str) {
                    events.push(IrEvent::ReasoningDelta(reasoning.to_string()));
                }

                if let Some(content) = delta.get("content").and_then(Value::as_str)
                    && !content.is_empty()
                {
                    events.push(IrEvent::TextDelta(content.to_string()));
                }

                if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for tool_call in tool_calls {
                        self.ingest_tool_call_delta(tool_call, &mut events)?;
                    }
                }
            }

            if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
                events.append(&mut self.flush_pending_calls());
                events.push(IrEvent::Finish(finish_reason_from_openai(finish_reason)));
            }
        }

        if let Some(usage) = parse_usage(value.get("usage")) {
            events.push(usage);
        }

        Ok(events)
    }

    /// Ingests one `delta.tool_calls` entry, buffering arguments until the
    /// calls complete.
    ///
    /// # Panics
    ///
    /// Panics when a newly started tool call cannot be buffered, violating
    /// the push-then-read invariant.
    fn ingest_tool_call_delta(
        &mut self,
        tool_call: &Value,
        events: &mut Vec<IrEvent>,
    ) -> Result<(), ApiError> {
        let index = tool_call
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or_default();

        let function = tool_call.get("function").cloned().unwrap_or(Value::Null);

        let call_id = tool_call.get("id").and_then(Value::as_str);
        let name = function.get("name").and_then(Value::as_str);
        let arguments_delta = function
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let pending = match self
            .pending_tool_calls
            .iter_mut()
            .find(|pending| pending.index as u64 == index)
        {
            Some(pending) => pending,
            None => {
                self.pending_tool_calls.push(PendingToolCall {
                    index: index as usize,
                    call_id: call_id.unwrap_or_default().to_string(),
                    name: name.unwrap_or_default().to_string(),
                    arguments: String::new(),
                });
                #[expect(clippy::expect_used, reason = "the entry was just pushed above")]
                self.pending_tool_calls
                    .last_mut()
                    .expect("entry was just pushed")
            }
        };

        if let Some(call_id) = call_id {
            pending.call_id = call_id.to_string();
        }

        if let Some(name) = name {
            pending.name = name.to_string();
        }

        pending.arguments.push_str(arguments_delta);

        events.push(IrEvent::ToolCallDelta {
            index: index as usize,
            call_id: call_id.map(str::to_string),
            name: name.map(str::to_string),
            arguments_delta: arguments_delta.to_string(),
        });

        Ok(())
    }

    /// Emits [`IrEvent::ToolCallDone`] for every pending call, marking them
    /// complete.
    fn flush_pending_calls(&mut self) -> Vec<IrEvent> {
        if self.has_flushed_calls {
            return Vec::new();
        }

        self.has_flushed_calls = true;

        self.pending_tool_calls
            .drain(..)
            .map(|pending| IrEvent::ToolCallDone {
                index: pending.index,
                call_id: pending.call_id,
                name: pending.name,
                arguments: pending.arguments,
                thought_signature: None,
            })
            .collect()
    }

    /// Emits [`IrEvent::MessageStart`] once per response.
    fn message_start_events(&mut self, value: &Value) -> Vec<IrEvent> {
        if self.is_started {
            return Vec::new();
        }

        self.is_started = true;

        vec![IrEvent::MessageStart {
            id: value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            model: value
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }]
    }
}

/// Extracts the primary choice, preferring `index == 0`; parallel choices are
/// not supported and the remainder is ignored.
fn first_choice(value: &Value) -> Option<&Value> {
    let choices = value.get("choices")?.as_array()?;

    choices
        .iter()
        .find(|choice| choice.get("index").and_then(Value::as_u64) == Some(0))
        .or_else(|| choices.first())
}

/// Converts an upstream usage object into an [`IrEvent::Usage`], if present.
fn parse_usage(usage: Option<&Value>) -> Option<IrEvent> {
    let usage = usage?;

    if !usage.is_object() {
        return None;
    }

    Some(IrEvent::Usage {
        input_tokens: usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

/// Extracts message text, reasoning, and complete tool calls from a full
/// upstream message object into IR events.
fn message_events_from_full(message: &Value) -> Vec<IrEvent> {
    let mut events = Vec::new();

    if let Some(reasoning) = message.get("reasoning").and_then(Value::as_str)
        && !reasoning.is_empty()
    {
        events.push(IrEvent::ReasoningDelta(reasoning.to_string()));
    }

    if let Some(content) = message.get("content").and_then(Value::as_str)
        && !content.is_empty()
    {
        events.push(IrEvent::TextDelta(content.to_string()));
    }

    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for (index, tool_call) in tool_calls.iter().enumerate() {
            let function = tool_call.get("function").cloned().unwrap_or(Value::Null);

            let call_id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            let name = function
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            events.push(IrEvent::ToolCallDelta {
                index,
                call_id: Some(call_id.clone()),
                name: Some(name.clone()),
                arguments_delta: arguments.clone(),
            });

            events.push(IrEvent::ToolCallDone {
                index,
                call_id,
                name,
                arguments,
                thought_signature: None,
            });
        }
    }

    events
}

/// Rejects an upstream body carrying a top-level error object.
fn reject_upstream_error(value: &Value) -> Result<(), ApiError> {
    let Some(error) = value.get("error") else {
        return Ok(());
    };

    let message = error
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| error.to_string());

    let status = error
        .get("code")
        .and_then(Value::as_u64)
        .map(|code| code.min(u64::from(u16::MAX)) as u16)
        .unwrap_or(crate::error::UPSTREAM_UNAVAILABLE_STATUS);

    Err(ApiError::upstream(status, message))
}

/// Streaming client response renderer producing Chat Completions SSE frames.
#[derive(Debug)]
pub(crate) struct SseRenderer {
    /// Gateway-issued completion identifier.
    id: String,

    /// Public model name echoed to the client.
    model: String,

    /// Unix creation timestamp.
    created: i64,

    /// Whether the leading role chunk was emitted.
    is_role_sent: bool,

    /// Whether the trailing usage chunk is wanted.
    include_usage: bool,
}

impl SseRenderer {
    /// Creates a renderer for one streamed completion.
    pub(crate) fn new(model: String, include_usage: bool) -> Self {
        Self {
            id: format!("chatcmpl-{}", new_uuid()),
            model,
            created: unix_time::unix_now(),
            is_role_sent: false,
            include_usage,
        }
    }

    /// Renders one IR event into zero or more SSE frames.
    pub(crate) fn render(&mut self, event: &IrEvent) -> Vec<String> {
        match event {
            IrEvent::MessageStart { .. } => {
                if self.is_role_sent {
                    Vec::new()
                } else {
                    self.is_role_sent = true;

                    vec![self.chunk_frame(
                        serde_json::json!({"role": "assistant", "content": ""}),
                        None,
                    )]
                }
            }

            IrEvent::TextDelta(delta) => {
                vec![self.chunk_frame(serde_json::json!({"content": delta}), None)]
            }

            IrEvent::ReasoningDelta(delta) => {
                vec![self.chunk_frame(serde_json::json!({"reasoning": delta}), None)]
            }

            IrEvent::ToolCallDelta {
                index,
                call_id,
                name,
                arguments_delta,
            } => {
                let mut function = serde_json::Map::new();

                if let Some(name) = name {
                    function.insert("name".to_string(), Value::String(name.clone()));
                }

                function.insert(
                    "arguments".to_string(),
                    Value::String(arguments_delta.clone()),
                );

                let mut tool_call = serde_json::Map::new();

                tool_call.insert("index".to_string(), Value::from(*index as u64));

                if let Some(call_id) = call_id {
                    tool_call.insert("id".to_string(), Value::String(call_id.clone()));
                    tool_call.insert("type".to_string(), Value::String("function".to_string()));
                }

                tool_call.insert("function".to_string(), Value::Object(function));

                vec![self.chunk_frame(
                    serde_json::json!({"tool_calls": [Value::Object(tool_call)]}),
                    None,
                )]
            }

            IrEvent::ToolCallDone {
                index,
                thought_signature: Some(signature),
                ..
            } => {
                vec![self.chunk_frame(
                    serde_json::json!({"tool_calls": [{
                        "index": index,
                        "thought_signature": signature,
                    }]}),
                    None,
                )]
            }

            IrEvent::ToolCallDone { .. } => Vec::new(),

            IrEvent::Finish(reason) => {
                vec![self.chunk_frame(
                    serde_json::json!({}),
                    Some(finish_reason_to_openai(*reason)),
                )]
            }

            IrEvent::Usage {
                input_tokens,
                output_tokens,
            } => {
                if !self.include_usage {
                    return Vec::new();
                }

                let usage = serde_json::json!({
                    "prompt_tokens": input_tokens,
                    "completion_tokens": output_tokens,
                    "total_tokens": input_tokens + output_tokens,
                });

                let chunk = serde_json::json!({
                    "id": self.id,
                    "object": "chat.completion.chunk",
                    "created": self.created,
                    "model": self.model,
                    "choices": [],
                    "usage": usage,
                });

                vec![format_data_frame(&chunk.to_string())]
            }

            IrEvent::Done => vec![format_data_frame("[DONE]")],
        }
    }

    /// Renders a mid-stream error as a terminal SSE frame.
    pub(crate) fn error_frame(&self, error: &ApiError) -> String {
        format_data_frame(&error.to_openai_json().to_string())
    }

    /// Formats one delta chunk with an optional finish reason.
    fn chunk_frame(&self, delta: Value, finish_reason: Option<&str>) -> String {
        let chunk = serde_json::json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": finish_reason,
            }],
        });

        format_data_frame(&chunk.to_string())
    }
}

/// Renders an aggregated completion as a non-streaming Chat Completions
/// response.
pub(crate) fn render_json_response(model: &str, aggregation: &Aggregation) -> Value {
    let tool_calls: Vec<Value> = aggregation
        .tool_calls
        .iter()
        .map(|tool_call| {
            let mut tool_call_json = serde_json::json!({
                "id": tool_call.call_id,
                "type": "function",
                "function": {
                    "name": tool_call.name,
                    "arguments": tool_call.arguments,
                },
            });

            if let Some(signature) = &tool_call.thought_signature {
                json_insert(
                    &mut tool_call_json,
                    "thought_signature",
                    Value::String(signature.clone()),
                );
            }

            tool_call_json
        })
        .collect();

    let mut message = serde_json::json!({"role": "assistant"});

    let content = if aggregation.text.is_empty() {
        Value::Null
    } else {
        Value::String(aggregation.text.clone())
    };

    json_insert(&mut message, "content", content);

    if !aggregation.reasoning.is_empty() {
        json_insert(
            &mut message,
            "reasoning",
            Value::String(aggregation.reasoning.clone()),
        );
    }

    if !tool_calls.is_empty() {
        json_insert(&mut message, "tool_calls", Value::Array(tool_calls));
    }

    let finish_reason = aggregation
        .finish
        .map(finish_reason_to_openai)
        .unwrap_or("stop");

    let (input_tokens, output_tokens) = aggregation.usage.unwrap_or((0, 0));

    serde_json::json!({
        "id": format!("chatcmpl-{}", new_uuid()),
        "object": "chat.completion",
        "created": unix_time::unix_now(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason,
        }],
        "usage": {
            "prompt_tokens": input_tokens,
            "completion_tokens": output_tokens,
            "total_tokens": input_tokens + output_tokens,
        },
    })
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "tests fail by panicking")]
#[expect(clippy::indexing_slicing, reason = "tests assert exact fixture shapes")]
mod tests {
    use super::{
        ParsedRequest, UpstreamParser, parse_request, render_json_response, render_request,
    };
    use crate::{
        error::ApiError,
        ir::{Aggregator, FinishReason, IrEvent, IrPart, IrRole},
        sse::{SseFrame, SseParser},
    };

    /// Parses a request from a JSON string.
    ///
    /// # Panics
    ///
    /// Panics if parsing fails.
    fn parse(json: &str) -> ParsedRequest {
        parse_request(json.as_bytes()).expect("request should parse")
    }

    /// Parses a request from a JSON string, expecting failure.
    ///
    /// # Panics
    ///
    /// Panics if parsing unexpectedly succeeds.
    fn parse_err(json: &str) -> ApiError {
        parse_request(json.as_bytes()).expect_err("request should fail")
    }

    /// Drains SSE frames through the upstream parser.
    ///
    /// # Panics
    ///
    /// Panics if frame parsing or ingestion fails.
    fn ingest_all(parser: &mut UpstreamParser, chunks: &[&str]) -> Vec<IrEvent> {
        let mut sse = SseParser::new();

        let mut events = Vec::new();

        for chunk in chunks {
            for frame in sse.ingest(chunk.as_bytes()).expect("frames should parse") {
                events.append(&mut parser.ingest_frame(&frame).expect("frame should ingest"));
            }
        }

        events
    }

    /// Verifies that parses conversation with images tools and reasoning holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.

    #[test]
    fn parses_conversation_with_images_tools_and_reasoning() {
        let parsed = parse(
            r#"{
                "model": "public-model",
                "messages": [
                    {"role": "system", "content": "be brief"},
                    {"role": "user", "content": [
                        {"type": "text", "text": "look"},
                        {"type": "image_url", "image_url": {"url": "data:image/png;base64,QUJD"}}
                    ]},
                    {"role": "assistant", "content": null, "tool_calls": [
                        {"id": "call_1", "type": "function",
                         "function": {"name": "weather", "arguments": "{\"city\":\"oslo\"}"},
                         "thought_signature": "sig1"}
                    ]},
                    {"role": "tool", "tool_call_id": "call_1", "content": "5 degrees"}
                ],
                "tools": [
                    {"type": "function", "function": {"name": "weather", "description": "d"}}
                ],
                "tool_choice": "auto",
                "temperature": 0.5,
                "top_p": 0.9,
                "max_completion_tokens": 32,
                "response_format": {"type": "json_object"},
                "reasoning_effort": "minimal",
                "stream": true,
                "stream_options": {"include_usage": true}
            }"#,
        );

        let ir = &parsed.ir;

        assert_eq!(ir.instructions.as_deref(), Some("be brief"));
        assert_eq!(ir.messages.len(), 3);
        assert_eq!(ir.messages[0].role, IrRole::User);
        assert_eq!(ir.messages[0].parts.len(), 2);
        assert_eq!(
            ir.messages[1].parts[0],
            IrPart::ToolCall {
                call_id: "call_1".to_string(),
                name: "weather".to_string(),
                arguments: "{\"city\":\"oslo\"}".to_string(),
                thought_signature: Some("sig1".to_string()),
            }
        );
        assert_eq!(ir.messages[2].role, IrRole::Tool);
        assert_eq!(ir.tools.len(), 1);
        assert_eq!(ir.max_output_tokens, Some(32));
        assert!(parsed.stream);
        assert!(parsed.include_usage);
    }

    /// Verifies that rejects unknown and unsupported fields holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.

    #[test]
    fn rejects_unknown_and_unsupported_fields() {
        let unknown = parse_err(r#"{"model":"m","messages":[],"bogus":1}"#);
        assert_eq!(unknown.status, 400);

        let unsupported_message_field =
            parse_err(r#"{"model":"m","messages":[{"role":"user","content":"x","seed":1}]}"#);
        assert_eq!(unsupported_message_field.param.as_deref(), Some("seed"));

        let both_budgets = parse_err(
            r#"{"model":"m","messages":[{"role":"user","content":"x"}],
               "max_tokens":1,"max_completion_tokens":2}"#,
        );
        assert_eq!(both_budgets.status, 400);

        let bad_effort = parse_err(
            r#"{"model":"m","messages":[{"role":"user","content":"x"}],
               "reasoning_effort":"turbo"}"#,
        );
        assert_eq!(bad_effort.status, 400);
    }

    /// Verifies that renders upstream request with tools and reasoning holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.

    #[test]
    fn renders_upstream_request_with_tools_and_reasoning() {
        let parsed = parse(
            r#"{
                "model": "public",
                "messages": [{"role": "user", "content": "hi"}],
                "tools": [{"type": "function", "function": {"name": "f"}}],
                "reasoning_effort": "high",
                "max_tokens": 9,
                "stream": false
            }"#,
        );

        let body = render_request(&parsed.ir, false);

        assert_eq!(body["model"], "public");
        assert_eq!(body["stream"], false);
        assert_eq!(body["max_completion_tokens"], 9);
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["tools"][0]["type"], "function");
    }

    /// Verifies that upstream stream parses deltas tool calls and usage holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.

    #[test]
    fn upstream_stream_parses_deltas_tool_calls_and_usage() {
        let mut parser = UpstreamParser::new();

        let events = ingest_all(
            &mut parser,
            &[
                concat!(
                    "data: {\"id\":\"up\",\"model\":\"m\",\"choices\":[",
                    "{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n"
                ),
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"he\"}}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning\":\"think\"}}]}\n\n",
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[",
                    "{\"index\":0,\"id\":\"call_1\",\"function\":",
                    "{\"name\":\"f\",\"arguments\":\"{}\"}}]}}]}\n\n"
                ),
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{},",
                    "\"finish_reason\":\"tool_calls\"}]}\n\n"
                ),
                "data: {\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\n",
                "data: [DONE]\n\n",
            ],
        );

        assert_eq!(
            events[0],
            IrEvent::MessageStart {
                id: "up".to_string(),
                model: "m".to_string(),
            }
        );

        assert!(events.contains(&IrEvent::TextDelta("he".to_string())));
        assert!(events.contains(&IrEvent::ReasoningDelta("think".to_string())));

        assert!(events.contains(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "f".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        }));

        assert!(events.contains(&IrEvent::Finish(FinishReason::ToolCall)));
        assert!(events.contains(&IrEvent::Usage {
            input_tokens: 3,
            output_tokens: 2,
        }));
    }

    /// Verifies that upstream json parses into events and aggregates holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.

    #[test]
    fn upstream_json_parses_into_events_and_aggregates() {
        let mut parser = UpstreamParser::new();

        let events = parser
            .parse_json_response(serde_json::json!({
                "id": "up", "model": "m",
                "choices": [{
                    "index": 0,
                    "finish_reason": "stop",
                    "message": {
                        "role": "assistant",
                        "content": "hello",
                        "reasoning": "hmm",
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {"name": "f", "arguments": "{}"}
                        }]
                    }
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 2}
            }))
            .expect("response should parse");

        let mut aggregator = Aggregator::default();

        for event in &events {
            aggregator.ingest(event);
        }

        let aggregation = aggregator.finish();

        assert_eq!(aggregation.text, "hello");
        assert_eq!(aggregation.reasoning, "hmm");
        assert_eq!(aggregation.tool_calls.len(), 1);
        assert_eq!(aggregation.finish, Some(FinishReason::Stop));
        assert_eq!(aggregation.usage, Some((1, 2)));

        let rendered = render_json_response("public", &aggregation);

        assert_eq!(rendered["object"], "chat.completion");
        assert_eq!(rendered["model"], "public");
        assert_eq!(rendered["choices"][0]["finish_reason"], "stop");
        assert_eq!(rendered["choices"][0]["message"]["content"], "hello");
        assert_eq!(rendered["usage"]["total_tokens"], 3);
    }

    /// Verifies that upstream error payload maps to upstream error holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.

    #[test]
    fn upstream_error_payload_maps_to_upstream_error() {
        let mut parser = UpstreamParser::new();

        let frame = SseFrame {
            event: None,
            data: r#"{"error": {"message": "overloaded", "code": 429}}"#.to_string(),
        };

        let error = parser.ingest_frame(&frame).expect_err("should fail");

        assert_eq!(error.status, 429);
        assert!(error.message.contains("overloaded"));
    }
}
