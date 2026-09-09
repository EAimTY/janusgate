//! OpenAI Chat Completions protocol codec.
//!
//! Serves the outward `POST /chat/completions` endpoint and speaks the
//! `openai_chat_completion` upstream protocol.

use super::{ClientFormat, ClientSseRenderer, ParsedRequest, UpstreamEventParser};
use crate::{
    error::{ApiError, invalid_request_from_json},
    format::{
        allowed_fields, arguments_as_object, finish_reason_from_openai, finish_reason_to_openai,
        json_insert, reject_unknown_fields, validate_content_part_fields,
    },
    ids::new_uuid,
    ir::{
        IrAggregation, IrEvent, IrFinishReason, IrImage, IrMessage, IrPart, IrReasoningEffort,
        IrRequest, IrResponseFormat, IrRole, IrTool, IrToolChoice, IrUsage,
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

/// Marker of the outward Chat Completions client format.
#[derive(Debug)]
pub(crate) struct Format;

impl ClientFormat for Format {
    fn parse_request(body: &[u8]) -> Result<ParsedRequest, ApiError> {
        parse_request(body)
    }

    fn sse_renderer(include_usage: bool, public_name: String) -> Box<dyn ClientSseRenderer + Send> {
        Box::new(SseRenderer::new(public_name, include_usage))
    }

    fn render_json_response(public_name: &str, aggregation: &IrAggregation) -> Value {
        render_json_response(public_name, aggregation)
    }
}

/// Parses a Chat Completions request body into IR.
///
/// # Errors
///
/// Returns [`ApiError`] for malformed JSON, unknown or unsupported fields,
/// invalid parameter ranges, and invalid image references.
fn parse_request(body: &[u8]) -> Result<ParsedRequest, ApiError> {
    let raw: Value = serde_json::from_slice(body).map_err(invalid_request_from_json)?;

    // Raw-field validation runs before deserialization so `raw` can be
    // consumed directly instead of cloned; deserializing a body with inline
    // base64 images can reach the 64 MB request cap.
    validate_message_fields(raw.get("messages"))?;
    validate_response_format_fields(raw.get("response_format"))?;

    let request: RequestBody = serde_json::from_value(raw).map_err(invalid_request_from_json)?;

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
                            return Err(ApiError::invalid_parameter(
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
                "auto" => IrToolChoice::Auto,
                "none" => IrToolChoice::None,
                "required" => IrToolChoice::Required,
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

                IrToolChoice::Function {
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
        Some(effort) => Some(IrReasoningEffort::parse_openai(effort).ok_or_else(|| {
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
fn validate_response_format_fields(response_format: Option<&Value>) -> Result<(), ApiError> {
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
                    .filter_map(|part| match part {
                        IrPart::Text { text } => {
                            Some(serde_json::json!({"type": "text", "text": text}))
                        }

                        IrPart::Image { image } => {
                            let url = image.to_url();

                            Some(
                                serde_json::json!({"type": "image_url", "image_url": {"url": url}}),
                            )
                        }

                        // The IR type system routes only text and images into
                        // user content.
                        _ => None,
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

                // The Chat Completions wire spec types assistant `content` as
                // `string | null`; parts arrays are not universally accepted
                // by compatible upstreams, and the gateway attaches no
                // per-part semantics to assistant text, so parts join like
                // system instructions do.
                let content = match texts.as_slice() {
                    [] => Value::Null,
                    texts => Value::String(
                        texts
                            .iter()
                            .map(|text| text.as_str())
                            .collect::<Vec<_>>()
                            .join("\n\n"),
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
            IrToolChoice::Auto => Value::String("auto".to_string()),
            IrToolChoice::None => Value::String("none".to_string()),
            IrToolChoice::Required => Value::String("required".to_string()),
            IrToolChoice::Function { name } => {
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
            .unwrap_or(IrFinishReason::Stop);

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
                if let Some(reasoning) = reasoning_field(delta)
                    && !reasoning.is_empty()
                {
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

impl UpstreamEventParser for UpstreamParser {
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, ApiError> {
        Self::ingest_frame(self, frame)
    }

    fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, ApiError> {
        Self::parse_json_response(self, value)
    }

    fn finish(&mut self) -> Vec<IrEvent> {
        // An upstream may close its stream cleanly after streaming tool-call
        // deltas without ever sending a finish reason or the `[DONE]`
        // sentinel; the buffered calls are complete and must be flushed.
        self.flush_pending_calls()
    }
}

/// Reads the reasoning text of an upstream delta or message object.
///
/// Both `reasoning` (OpenRouter, vLLM) and `reasoning_content` (the DeepSeek
/// de-facto standard for OpenAI-compatible reasoners) are accepted; when both
/// keys are present, `reasoning` wins.
fn reasoning_field(value: &Value) -> Option<&str> {
    value
        .get("reasoning")
        .and_then(Value::as_str)
        .or_else(|| value.get("reasoning_content").and_then(Value::as_str))
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

    if let Some(reasoning) = reasoning_field(message)
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
///
/// An error body inside a 2xx response is a malformed upstream payload
/// (README §6.7), so the embedded `code` is never mirrored as the outward
/// status — a 2xx code would otherwise render a success status carrying an
/// error body. The embedded code and text ride in the message instead.
fn reject_upstream_error(value: &Value) -> Result<(), ApiError> {
    let Some(error) = value.get("error") else {
        return Ok(());
    };

    let text = error
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| error.to_string());

    let detail = match error.get("code") {
        Some(code) => format!("{text} (upstream error code: {code})"),
        None => text,
    };

    Err(ApiError::upstream_unavailable(detail))
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

    /// Latest token accounting observed; rendered once as the terminal
    /// usage chunk, because the official stream shape carries usage only on
    /// the final chunk before `[DONE]`.
    usage: Option<IrUsage>,
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
            usage: None,
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
                // Chat Completions has no official reasoning delta field;
                // `reasoning_content` is the de-facto key that reasoner-aware
                // OpenAI-compatible clients read (see DeepSeek's reasoning
                // model guide).
                vec![self.chunk_frame(serde_json::json!({"reasoning_content": delta}), None)]
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
                // Gemini upstreams attach cumulative usage to every stream
                // chunk; the latest observation wins and renders once,
                // terminally.
                self.usage = Some(IrUsage {
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                });

                Vec::new()
            }

            IrEvent::Done => {
                let mut frames = Vec::new();

                if self.include_usage
                    && let Some(usage) = self.usage
                {
                    frames.push(self.usage_frame(usage));
                }

                frames.push(format_data_frame("[DONE]"));

                frames
            }
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

    /// Formats the terminal usage-only chunk carrying an empty choices
    /// array, matching the official stream shape.
    fn usage_frame(&self, usage: IrUsage) -> String {
        let chunk = serde_json::json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [],
            "usage": {
                "prompt_tokens": usage.input_tokens,
                "completion_tokens": usage.output_tokens,
                "total_tokens": usage.input_tokens + usage.output_tokens,
            },
        });

        format_data_frame(&chunk.to_string())
    }
}

impl ClientSseRenderer for SseRenderer {
    fn render(&mut self, event: &IrEvent) -> Vec<String> {
        Self::render(self, event)
    }

    fn error_frame(&mut self, error: &ApiError) -> String {
        Self::error_frame(self, error)
    }
}

/// Renders an aggregated completion as a non-streaming Chat Completions
/// response.
pub(crate) fn render_json_response(model: &str, aggregation: &IrAggregation) -> Value {
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
            "reasoning_content",
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

    let usage = aggregation.usage.unwrap_or_default();

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
            "prompt_tokens": usage.input_tokens,
            "completion_tokens": usage.output_tokens,
            "total_tokens": usage.input_tokens + usage.output_tokens,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ParsedRequest, SseRenderer, UpstreamParser, parse_request, render_json_response,
        render_request,
    };
    use crate::{
        error::ApiError,
        format::UpstreamEventParser as _,
        ir::{Aggregator, IrEvent, IrFinishReason, IrPart, IrRole, IrUsage},
        sse::{SseFrame, SseParser},
    };
    use serde_json::Value;

    /// Parses a request from a JSON string.
    ///
    /// # Panics
    ///
    /// Panics if parsing fails.
    fn parse(json: &str) -> ParsedRequest {
        #[expect(
            clippy::expect_used,
            reason = "the test body is a valid request fixture"
        )]
        let parsed = parse_request(json.as_bytes()).expect("request should parse");

        parsed
    }

    /// Parses a request from a JSON string, expecting failure.
    ///
    /// # Panics
    ///
    /// Panics if parsing unexpectedly succeeds.
    fn parse_err(json: &str) -> ApiError {
        #[expect(clippy::expect_used, reason = "the test asserts this exact rejection")]
        let error = parse_request(json.as_bytes()).expect_err("request should fail");

        error
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
            #[expect(clippy::expect_used, reason = "every fixture chunk is well-formed SSE")]
            for frame in sse.ingest(chunk.as_bytes()).expect("frames should parse") {
                #[expect(
                    clippy::expect_used,
                    reason = "every fixture frame is a valid chat payload"
                )]
                events.append(&mut parser.ingest_frame(&frame).expect("frame should ingest"));
            }
        }

        events
    }

    /// Parses a conversation containing images, tools, and reasoning.
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

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so indexed messages are
        // read through annotated bindings first.
        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture parses exactly three messages, asserted above"
        )]
        let first_message = &ir.messages[0];

        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture's assistant message carries exactly one part"
        )]
        let assistant_part = &ir.messages[1].parts[0];

        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture parses exactly three messages, asserted above"
        )]
        let tool_message = &ir.messages[2];

        assert_eq!(first_message.role, IrRole::User);
        assert_eq!(first_message.parts.len(), 2);
        assert_eq!(
            assistant_part,
            &IrPart::ToolCall {
                call_id: "call_1".to_string(),
                name: "weather".to_string(),
                arguments: "{\"city\":\"oslo\"}".to_string(),
                thought_signature: Some("sig1".to_string()),
            }
        );
        assert_eq!(tool_message.role, IrRole::Tool);
        assert_eq!(ir.tools.len(), 1);
        assert_eq!(ir.max_output_tokens, Some(32));
        assert!(parsed.stream);
        assert!(parsed.include_usage);
    }

    /// Rejects unknown and unsupported request fields.
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

    /// Renders an upstream request carrying tools and reasoning.
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

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so rendered fields are
        // read through annotated bindings first.
        #[expect(
            clippy::indexing_slicing,
            reason = "render_request always emits the asserted upstream fields"
        )]
        let first_tool_type = &body["tools"][0]["type"];

        #[expect(
            clippy::indexing_slicing,
            reason = "render_request always emits the asserted upstream fields"
        )]
        let model = &body["model"];

        #[expect(
            clippy::indexing_slicing,
            reason = "render_request always emits the asserted upstream fields"
        )]
        let stream = &body["stream"];

        #[expect(
            clippy::indexing_slicing,
            reason = "render_request always emits the asserted upstream fields"
        )]
        let max_completion_tokens = &body["max_completion_tokens"];

        #[expect(
            clippy::indexing_slicing,
            reason = "render_request always emits the asserted upstream fields"
        )]
        let reasoning_effort = &body["reasoning_effort"];

        assert_eq!(model, "public");
        assert_eq!(stream, false);
        assert_eq!(max_completion_tokens, 9);
        assert_eq!(reasoning_effort, "high");
        assert_eq!(first_tool_type, "function");
    }

    /// Parses an upstream stream of deltas, tool calls, and usage.
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

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so the leading event is
        // read through an annotated binding first.
        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture's first frame emits MessageStart first"
        )]
        let first_event = &events[0];

        assert_eq!(
            first_event,
            &IrEvent::MessageStart {
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

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::ToolCall)));
        assert!(events.contains(&IrEvent::Usage {
            input_tokens: 3,
            output_tokens: 2,
        }));
    }

    /// Aggregates a streamed tool call completed by the flush at the finish
    /// chunk, proving the streaming done path feeds aggregation.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_stream_aggregates_tool_calls_from_done_event() {
        let mut parser = UpstreamParser::new();

        let events = ingest_all(
            &mut parser,
            &[
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[",
                    "{\"index\":0,\"id\":\"call_1\",\"function\":",
                    "{\"name\":\"f\",\"arguments\":\"12\"}}]}}]}\n\n"
                ),
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[",
                    "{\"index\":0,\"function\":{\"arguments\":\"34\"}}]}}]}\n\n"
                ),
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{},",
                    "\"finish_reason\":\"tool_calls\"}]}\n\n"
                ),
            ],
        );

        let mut aggregator = Aggregator::default();

        for event in &events {
            aggregator.ingest(event);
        }

        let aggregation = aggregator.finish();

        assert_eq!(
            aggregation
                .tool_calls
                .first()
                .map(|call| (call.name.as_str(), call.arguments.as_str())),
            Some(("f", "1234"))
        );
    }

    /// Parses an upstream JSON response into events and aggregates them.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_json_parses_into_events_and_aggregates() {
        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the fixture is a valid chat completion body"
        )]
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
        assert_eq!(aggregation.finish, Some(IrFinishReason::Stop));

        assert_eq!(
            aggregation.usage,
            Some(IrUsage {
                input_tokens: 1,
                output_tokens: 2,
            })
        );

        let rendered = render_json_response("public", &aggregation);

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so rendered fields are
        // read through annotated bindings first.
        #[expect(
            clippy::indexing_slicing,
            reason = "render_json_response always emits the asserted completion fields"
        )]
        let object = &rendered["object"];

        #[expect(
            clippy::indexing_slicing,
            reason = "render_json_response always emits the asserted completion fields"
        )]
        let model = &rendered["model"];

        #[expect(
            clippy::indexing_slicing,
            reason = "render_json_response always emits the asserted completion fields"
        )]
        let first_choice = &rendered["choices"][0];

        let first_choice_message = &first_choice["message"];

        #[expect(
            clippy::indexing_slicing,
            reason = "render_json_response always emits the usage object"
        )]
        let total_tokens = &rendered["usage"]["total_tokens"];

        assert_eq!(object, "chat.completion");
        assert_eq!(model, "public");

        let finish_reason = &first_choice["finish_reason"];

        let content = &first_choice_message["content"];

        assert_eq!(finish_reason, "stop");
        assert_eq!(content, "hello");
        assert_eq!(total_tokens, 3);
    }

    /// Maps an embedded upstream error payload to a 502 upstream error whose
    /// message keeps the upstream code and text, never mirroring the embedded
    /// code as the outward status (README §6.7).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_error_payload_maps_to_502_upstream_error() {
        let mut parser = UpstreamParser::new();

        let frame = SseFrame {
            event: None,
            data: r#"{"error": {"message": "overloaded", "code": 429}}"#.to_string(),
        };

        #[expect(clippy::expect_used, reason = "the test asserts this exact rejection")]
        let error = parser.ingest_frame(&frame).expect_err("should fail");

        assert_eq!(error.status, 502);
        assert_eq!(error.code.as_deref(), Some("upstream_error"));
        assert!(error.message.contains("overloaded"));
        assert!(error.message.contains("429"));

        // A nonsense 2xx embedded code must not render a success status.
        let frame = SseFrame {
            event: None,
            data: r#"{"error": {"message": "odd", "code": 200}}"#.to_string(),
        };

        #[expect(clippy::expect_used, reason = "the test asserts this exact rejection")]
        let error = parser.ingest_frame(&frame).expect_err("should fail");

        assert_eq!(error.status, 502);
    }

    /// Rejects assistant tool-call arguments that are not a JSON object,
    /// matching the strictness the Responses endpoint applies to replayed
    /// `function_call` items.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn rejects_non_object_tool_call_arguments() {
        let error = parse_err(
            r#"{"model":"m","messages":[{"role":"assistant","content":null,"tool_calls":[
                {"id":"call_1","type":"function","function":{"name":"f","arguments":"not json"}}
            ]}]}"#,
        );

        assert_eq!(error.status, 400);
        assert_eq!(error.param.as_deref(), Some("arguments"));
    }

    /// Flushes pending tool calls at a clean end of stream that carries
    /// neither a finish reason nor a `[DONE]` sentinel.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn finish_flushes_pending_tool_calls_on_clean_eof() {
        let mut parser = UpstreamParser::new();

        let mut events = ingest_all(
            &mut parser,
            &[concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[",
                "{\"index\":0,\"id\":\"call_1\",\"function\":",
                "{\"name\":\"f\",\"arguments\":\"{}\"}}]}}]}\n\n"
            )],
        );

        events.append(&mut parser.finish());

        assert!(events.contains(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "f".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        }));

        // The flush is one-shot: a stream whose calls already completed
        // yields nothing further.
        assert!(parser.finish().is_empty());
    }

    /// Renders assistant content as a plain string.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn renders_assistant_content_as_plain_string() {
        let parsed = parse(
            r#"{
                "model": "public",
                "messages": [
                    {"role": "user", "content": "hi"},
                    {"role": "assistant", "content": "previous answer"},
                    {"role": "assistant", "content": [
                        {"type": "text", "text": "a"},
                        {"type": "text", "text": "b"}
                    ]}
                ]
            }"#,
        );

        let body = render_request(&parsed.ir, false);

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so rendered messages are
        // read through annotated bindings first.
        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture parses exactly three messages"
        )]
        let assistant_message = &body["messages"][1];

        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture parses exactly three messages"
        )]
        let second_assistant_message = &body["messages"][2];

        let role = &assistant_message["role"];

        let content = &assistant_message["content"];

        let joined_content = &second_assistant_message["content"];

        assert_eq!(role, "assistant");
        assert_eq!(content, "previous answer");
        assert_eq!(joined_content, "a\n\nb");
    }

    /// Parses the upstream `reasoning_content` key as reasoning text.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_parses_reasoning_content_key() {
        let mut parser = UpstreamParser::new();

        let events = ingest_all(
            &mut parser,
            &[concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":",
                "{\"reasoning_content\":\"think\"}}]}\n\n"
            )],
        );

        assert!(events.contains(&IrEvent::ReasoningDelta("think".to_string())));
    }

    /// Prefers `reasoning` over `reasoning_content` when both are present.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_prefers_reasoning_over_reasoning_content() {
        let mut parser = UpstreamParser::new();

        let events = ingest_all(
            &mut parser,
            &[concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":",
                "{\"reasoning\":\"a\",\"reasoning_content\":\"b\"}}]}\n\n"
            )],
        );

        let reasoning_deltas: Vec<&String> = events
            .iter()
            .filter_map(|event| match event {
                IrEvent::ReasoningDelta(delta) => Some(delta),
                _ => None,
            })
            .collect();

        assert_eq!(reasoning_deltas, vec![&"a".to_string()]);
    }

    /// Parses `reasoning_content` from an upstream JSON message and emits the
    /// same key when rendering the aggregated client response.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_json_parses_reasoning_content_and_render_emits_it() {
        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the fixture is a valid chat completion body"
        )]
        let events = parser
            .parse_json_response(serde_json::json!({
                "id": "up", "model": "m",
                "choices": [{
                    "index": 0,
                    "finish_reason": "stop",
                    "message": {"role": "assistant", "content": "hello",
                                "reasoning_content": "hmm"}
                }]
            }))
            .expect("response should parse");

        assert!(events.contains(&IrEvent::ReasoningDelta("hmm".to_string())));

        let mut aggregator = Aggregator::default();

        for event in &events {
            aggregator.ingest(event);
        }

        let rendered = render_json_response("public", &aggregator.finish());

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so rendered fields are
        // read through annotated bindings first.
        #[expect(
            clippy::indexing_slicing,
            reason = "render_json_response always emits the first choice's message"
        )]
        let reasoning_content = &rendered["choices"][0]["message"]["reasoning_content"];

        assert_eq!(reasoning_content, "hmm");
    }

    /// Verifies that an assistant message replaying the gateway's own
    /// `reasoning_content` field is accepted and the field ignored,
    /// symmetrically with `reasoning`.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn accepts_reasoning_content_on_assistant_messages() {
        let parsed = parse(
            r#"{
                "model": "public",
                "messages": [
                    {"role": "assistant", "content": "a", "reasoning_content": "think"},
                    {"role": "assistant", "content": "b", "reasoning": "think"}
                ]
            }"#,
        );

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so indexed messages are
        // read through annotated bindings first.
        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture parses exactly two messages"
        )]
        let first_message = &parsed.ir.messages[0];

        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture parses exactly two messages"
        )]
        let second_message = &parsed.ir.messages[1];

        assert_eq!(
            first_message.parts,
            vec![IrPart::Text {
                text: "a".to_string()
            }]
        );

        assert_eq!(
            second_message.parts,
            vec![IrPart::Text {
                text: "b".to_string()
            }]
        );
    }

    /// Emits the `reasoning_content` key on client streaming deltas.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn client_stream_emits_reasoning_content_key() {
        let mut renderer = SseRenderer::new("public".to_string(), false);

        #[expect(
            clippy::expect_used,
            reason = "a reasoning delta renders exactly one frame"
        )]
        let frame = renderer
            .render(&IrEvent::ReasoningDelta("think".to_string()))
            .pop()
            .expect("reasoning delta renders a frame");

        #[expect(
            clippy::expect_used,
            reason = "the renderer emits `data: ` prefixed frames"
        )]
        let payload: Value = serde_json::from_str(
            frame
                .strip_prefix("data: ")
                .expect("data frame prefix")
                .trim_end(),
        )
        .expect("payload is JSON");

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so rendered fields are
        // read through annotated bindings first.
        #[expect(
            clippy::indexing_slicing,
            reason = "chat chunks always carry a single choices entry"
        )]
        let reasoning_content = &payload["choices"][0]["delta"]["reasoning_content"];

        assert_eq!(reasoning_content, "think");
    }

    /// Renders events through a chat client renderer, returning raw SSE
    /// frames.
    fn render_chat_client(include_usage: bool, events: &[IrEvent]) -> Vec<String> {
        let mut renderer = SseRenderer::new("public".to_string(), include_usage);

        let mut frames = Vec::new();

        for event in events {
            frames.append(&mut renderer.render(event));
        }

        frames
    }

    /// Emits exactly one usage chunk — carrying the last observed usage
    /// event — immediately before `[DONE]`, no matter how many upstream
    /// usage events arrive; Gemini upstreams attach cumulative usage to
    /// every stream chunk.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn client_stream_emits_one_terminal_usage_chunk() {
        let events = [
            IrEvent::MessageStart {
                id: "up".to_string(),
                model: "m".to_string(),
            },
            IrEvent::TextDelta("a".to_string()),
            IrEvent::Usage {
                input_tokens: 2,
                output_tokens: 3,
            },
            IrEvent::Usage {
                input_tokens: 5,
                output_tokens: 7,
            },
            IrEvent::Finish(IrFinishReason::Stop),
            IrEvent::Done,
        ];

        let frames = render_chat_client(true, &events);

        assert_eq!(frames.last().map(String::as_str), Some("data: [DONE]\n\n"));

        let usage_payloads: Vec<Value> = frames
            .iter()
            .filter_map(|frame| {
                let payload: Value =
                    serde_json::from_str(frame.strip_prefix("data: ")?.trim_end()).ok()?;

                payload
                    .get("usage")
                    .filter(|usage| !usage.is_null())
                    .cloned()
            })
            .collect();

        assert_eq!(usage_payloads.len(), 1);

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so the terminal usage
        // chunk's fields are read through annotated bindings first.
        #[expect(
            clippy::indexing_slicing,
            reason = "the single usage chunk was asserted above"
        )]
        let usage = &usage_payloads[0];

        let prompt_tokens = &usage["prompt_tokens"];

        let completion_tokens = &usage["completion_tokens"];

        let total_tokens = &usage["total_tokens"];

        assert_eq!(prompt_tokens, 5);
        assert_eq!(completion_tokens, 7);
        assert_eq!(total_tokens, 12);

        // The usage chunk sits directly before the sentinel and carries the
        // official shape: an empty choices array and the chunk envelope.
        #[expect(clippy::indexing_slicing, reason = "the sentinel was asserted above")]
        let usage_frame = &frames[frames.len() - 2];

        #[expect(
            clippy::expect_used,
            reason = "the renderer emits `data: ` prefixed frames"
        )]
        let usage_chunk: Value = serde_json::from_str(
            usage_frame
                .strip_prefix("data: ")
                .expect("data frame prefix")
                .trim_end(),
        )
        .expect("usage chunk is JSON");

        #[expect(
            clippy::indexing_slicing,
            reason = "the renderer emits the chunk envelope fields"
        )]
        let chunk_object = &usage_chunk["object"];

        #[expect(
            clippy::indexing_slicing,
            reason = "the renderer emits the chunk envelope fields"
        )]
        let chunk_choices = &usage_chunk["choices"];

        assert_eq!(chunk_object, "chat.completion.chunk");
        assert_eq!(chunk_choices, &Value::Array(Vec::new()));
    }

    /// Keeps the `include_usage` gating exact: without the option no frame
    /// carries a usage field at all.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn client_stream_without_include_usage_stays_usage_free() {
        let events = [
            IrEvent::MessageStart {
                id: "up".to_string(),
                model: "m".to_string(),
            },
            IrEvent::TextDelta("a".to_string()),
            IrEvent::Usage {
                input_tokens: 2,
                output_tokens: 3,
            },
            IrEvent::Finish(IrFinishReason::Stop),
            IrEvent::Done,
        ];

        let frames = render_chat_client(false, &events);

        assert_eq!(frames.last().map(String::as_str), Some("data: [DONE]\n\n"));

        for frame in &frames {
            let Some(data) = frame.strip_prefix("data: ") else {
                continue;
            };

            if data.trim_end() == "[DONE]" {
                continue;
            }

            #[expect(clippy::expect_used, reason = "every rendered frame is valid JSON")]
            let payload: Value = serde_json::from_str(data.trim_end()).expect("chunk is JSON");

            assert!(payload.get("usage").is_none());
        }
    }
}
