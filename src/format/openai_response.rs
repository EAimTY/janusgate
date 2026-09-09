//! OpenAI Responses protocol codec.
//!
//! Serves the outward `POST /responses` endpoint and speaks the
//! `openai_response` upstream protocol (including the Codex backend).

use crate::{
    error::{ApiError, invalid_request_from_json},
    format::{arguments_as_object, json_insert},
    ids::new_uuid,
    ir::{
        Aggregation, FinishReason, IrEvent, IrImage, IrMessage, IrPart, IrRequest,
        IrResponseFormat, IrRole, IrTool, ReasoningEffort, ToolChoice,
    },
    sse::{SseFrame, format_frame},
    unix_time,
};
use serde::Deserialize;
use serde_json::Value;

/// Fields allowed on each Responses input item, keyed by item type. Unknown
/// fields are rejected because internally tagged serde enums cannot enforce
/// this.
const INPUT_ITEM_FIELDS: &[(&str, &[&str])] = &[
    ("message", &["type", "role", "content"]),
    (
        "function_call",
        &[
            "type",
            "id",
            "call_id",
            "name",
            "arguments",
            "status",
            "thought_signature",
        ],
    ),
    (
        "function_call_output",
        &["type", "id", "call_id", "output", "status"],
    ),
];

/// Fields allowed on each Responses content part, keyed by part type.
const CONTENT_PART_FIELDS: &[(&str, &[&str])] = &[
    ("input_text", &["type", "text"]),
    ("output_text", &["type", "text"]),
    ("input_image", &["type", "image_url"]),
];

/// Strictly parsed Responses request body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestBody {
    /// Public model name to route to.
    model: String,

    /// Conversation input.
    input: InputIn,

    /// System instructions.
    #[serde(default)]
    instructions: Option<String>,

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

    /// Output token budget.
    #[serde(default)]
    max_output_tokens: Option<u64>,

    /// Reasoning configuration.
    #[serde(default)]
    reasoning: Option<ReasoningIn>,

    /// Whether the client wants a streamed response.
    #[serde(default)]
    stream: Option<bool>,

    /// Storage request; the gateway is stateless, so only `false` is legal.
    #[serde(default)]
    store: Option<bool>,

    /// Text output configuration.
    #[serde(default)]
    text: Option<TextIn>,
}

/// Conversation input: plain string or item array.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum InputIn {
    /// Single user text turn.
    Text(String),

    /// Ordered input items.
    Items(Vec<ItemIn>),
}

/// Input item, tagged by `type`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ItemIn {
    /// Conversation message.
    Message {
        /// Authoring role.
        role: String,

        /// Text or typed content.
        content: MessageContentIn,
    },

    /// A past tool invocation.
    FunctionCall {
        /// Invocation identifier.
        call_id: String,

        /// Function name.
        name: String,

        /// JSON-encoded arguments.
        #[serde(default)]
        arguments: String,

        /// Thought signature to reattach across turns.
        #[serde(default)]
        thought_signature: Option<String>,
    },

    /// A tool result.
    FunctionCallOutput {
        /// Identifier of the invocation being answered.
        call_id: String,

        /// Result content.
        output: String,
    },

    /// Reasoning items carry no replayable state and are accepted but ignored.
    Reasoning {},
}

/// Message content: plain string or typed parts.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum MessageContentIn {
    /// Plain text.
    Text(String),

    /// Typed parts.
    Parts(Vec<PartIn>),
}

/// Content part, tagged by `type`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PartIn {
    /// Text segment on the input side.
    InputText {
        /// The text content.
        text: String,
    },

    /// Text segment on the assistant side.
    OutputText {
        /// The text content.
        text: String,
    },

    /// Image segment.
    InputImage {
        /// Remote or data URL of the image.
        image_url: String,
    },
}

/// Advertised function tool.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolIn {
    /// Tool kind; only `function` is supported.
    #[serde(rename = "type")]
    tool_type: String,

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

/// Tool choice: a named mode or a forced function.
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

        /// Required function name.
        name: String,
    },
}

/// Reasoning configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReasoningIn {
    /// OpenAI reasoning effort.
    #[serde(default)]
    effort: Option<String>,

    /// Summary verbosity; only `auto` is accepted.
    #[serde(default)]
    summary: Option<String>,
}

/// Text output configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TextIn {
    /// Response format restriction.
    #[serde(default)]
    format: Option<FormatIn>,
}

/// Response format, tagged by `type`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum FormatIn {
    /// Free-form text.
    Text,

    /// Any valid JSON value.
    JsonObject,

    /// JSON conforming to a named schema.
    JsonSchema {
        /// Schema name.
        name: String,

        /// The JSON Schema itself.
        schema: Value,

        /// OpenAI strictness flag; accepted and ignored.
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        strict: Option<bool>,
    },
}

/// Client request parsed into IR with endpoint options.
#[derive(Debug)]
pub(crate) struct ParsedRequest {
    /// Format-independent request.
    pub(crate) ir: IrRequest,

    /// Whether the client requested streaming.
    pub(crate) stream: bool,
}

/// Parses a Responses request body into IR.
///
/// # Errors
///
/// Returns [`ApiError`] for malformed JSON, unknown or unsupported fields,
/// invalid parameter ranges, and invalid image references.
pub(crate) fn parse_request(body: &[u8]) -> Result<ParsedRequest, ApiError> {
    let raw: Value = serde_json::from_slice(body).map_err(invalid_request_from_json)?;
    let request: RequestBody =
        serde_json::from_value(raw.clone()).map_err(invalid_request_from_json)?;

    validate_input_item_fields(raw.get("input"))?;

    if request.store == Some(true) {
        return Err(ApiError::invalid_parameter(
            "store",
            "The gateway is stateless; store must be false",
        ));
    }

    if let Some(reasoning) = &request.reasoning
        && let Some(summary) = &reasoning.summary
        && summary != "auto"
    {
        return Err(ApiError::invalid_parameter(
            "summary",
            format!("Unsupported reasoning summary `{summary}`"),
        ));
    }

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

    let mut instruction_parts: Vec<String> = Vec::new();
    let mut messages: Vec<IrMessage> = Vec::new();

    let input_items: Vec<ItemIn> = match request.input {
        InputIn::Text(text) => vec![ItemIn::Message {
            role: "user".to_string(),
            content: MessageContentIn::Text(text),
        }],

        InputIn::Items(items) => items,
    };

    for item in input_items {
        match item {
            ItemIn::Message { role, content } => match role.as_str() {
                "system" | "developer" => {
                    if let Some(text) = message_content_text(&content) {
                        instruction_parts.push(text);
                    }
                }

                "user" => messages.push(IrMessage {
                    role: IrRole::User,
                    parts: message_content_parts(&content)?,
                }),

                "assistant" => messages.push(IrMessage {
                    role: IrRole::Assistant,
                    parts: message_content_parts(&content)?,
                }),

                other => {
                    return Err(ApiError::invalid_parameter(
                        "role",
                        format!("Unknown input message role `{other}`"),
                    ));
                }
            },

            ItemIn::FunctionCall {
                call_id,
                name,
                arguments,
                thought_signature,
            } => {
                arguments_as_object(&arguments)?;

                messages.push(IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![IrPart::ToolCall {
                        call_id,
                        name,
                        arguments,
                        thought_signature,
                    }],
                });
            }

            ItemIn::FunctionCallOutput { call_id, output } => {
                messages.push(IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id,
                        content: output,
                    }],
                });
            }

            ItemIn::Reasoning {} => {}
        }
    }

    let instructions = build_instructions(request.instructions, instruction_parts);

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
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.clone(),
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

            ToolChoiceIn::Function { tool_type, name } => {
                if tool_type != "function" {
                    return Err(ApiError::invalid_parameter(
                        "type",
                        format!("Unsupported tool_choice type `{tool_type}`"),
                    ));
                }

                ToolChoice::Function { name: name.clone() }
            }
        }),
    };

    let response_format = match request.text.as_ref().and_then(|text| text.format.as_ref()) {
        None => None,
        Some(FormatIn::Text) => Some(IrResponseFormat::Text),
        Some(FormatIn::JsonObject) => Some(IrResponseFormat::Json),
        Some(FormatIn::JsonSchema {
            name,
            schema,
            strict: _,
        }) => Some(IrResponseFormat::JsonSchema {
            name: name.clone(),
            schema: schema.clone(),
        }),
    };

    let reasoning_effort = match request
        .reasoning
        .as_ref()
        .and_then(|reasoning| reasoning.effort.as_ref())
    {
        None => None,
        Some(effort) => Some(ReasoningEffort::parse_openai(effort).ok_or_else(|| {
            ApiError::invalid_parameter("effort", format!("Unknown reasoning effort `{effort}`"))
        })?),
    };

    Ok(ParsedRequest {
        ir: IrRequest {
            model: request.model,
            instructions,
            messages,
            tools,
            tool_choice,
            temperature: request.temperature,
            top_p: request.top_p,
            max_output_tokens: request.max_output_tokens,
            response_format,
            reasoning_effort,
        },
        stream: request.stream.unwrap_or(false),
    })
}

/// Combines the `instructions` field with system/developer message texts in
/// encounter order.
fn build_instructions(direct: Option<String>, pushed: Vec<String>) -> Option<String> {
    let mut combined = Vec::with_capacity(pushed.len() + 1);

    if let Some(direct) = direct {
        combined.push(direct);
    }

    combined.extend(pushed);

    if combined.is_empty() {
        None
    } else {
        Some(combined.join("\n\n"))
    }
}

/// Extracts plain text from message content, when the content is purely text.
fn message_content_text(content: &MessageContentIn) -> Option<String> {
    match content {
        MessageContentIn::Text(text) => Some(text.clone()),

        MessageContentIn::Parts(parts) => {
            let mut combined = String::new();

            for part in parts {
                match part {
                    PartIn::InputText { text } | PartIn::OutputText { text } => {
                        if !combined.is_empty() {
                            combined.push_str("\n\n");
                        }

                        combined.push_str(text);
                    }

                    PartIn::InputImage { .. } => return None,
                }
            }

            Some(combined)
        }
    }
}

/// Converts message content into IR parts.
///
/// # Errors
///
/// Returns [`ApiError`] for malformed image references.
fn message_content_parts(content: &MessageContentIn) -> Result<Vec<IrPart>, ApiError> {
    match content {
        MessageContentIn::Text(text) => Ok(vec![IrPart::Text { text: text.clone() }]),

        MessageContentIn::Parts(parts) => {
            let mut converted = Vec::with_capacity(parts.len());

            for part in parts {
                converted.push(match part {
                    PartIn::InputText { text } | PartIn::OutputText { text } => {
                        IrPart::Text { text: text.clone() }
                    }

                    PartIn::InputImage { image_url } => IrPart::Image {
                        image: parse_image_url(image_url)?,
                    },
                });
            }

            Ok(converted)
        }
    }
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

/// Rejects unknown fields on Responses input items; internally tagged serde
/// enums cannot enforce this themselves.
///
/// # Errors
///
/// Returns [`ApiError`] naming the first unknown field.
pub(crate) fn validate_input_item_fields(input: Option<&Value>) -> Result<(), ApiError> {
    let items = match input {
        Some(Value::Array(items)) => items,
        _ => return Ok(()),
    };

    for item in items {
        let Some(object) = item.as_object() else {
            continue;
        };

        let item_type = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if item_type == "reasoning" {
            continue;
        }

        let Some((_, allowed_fields)) = INPUT_ITEM_FIELDS
            .iter()
            .find(|(name, _)| *name == item_type)
        else {
            continue;
        };

        for field in object.keys() {
            if !allowed_fields.contains(&field.as_str()) {
                return Err(ApiError::invalid_parameter(
                    field,
                    format!("Unsupported field `{field}` in `{item_type}` input item"),
                ));
            }
        }

        if let Some(parts) = object.get("content").and_then(Value::as_array) {
            for part in parts {
                let Some(part_object) = part.as_object() else {
                    continue;
                };

                let part_type = part_object
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();

                let Some((_, allowed_fields)) = CONTENT_PART_FIELDS
                    .iter()
                    .find(|(name, _)| *name == part_type)
                else {
                    continue;
                };

                for field in part_object.keys() {
                    if !allowed_fields.contains(&field.as_str()) {
                        return Err(ApiError::invalid_parameter(
                            field,
                            format!("Unsupported field `{field}` in `{part_type}` content part"),
                        ));
                    }
                }
            }
        }
    }

    Ok(())
}

/// Renders an IR request into an upstream Responses payload.
pub(crate) fn render_request(ir: &IrRequest, stream: bool) -> Value {
    let mut input: Vec<Value> = Vec::new();

    for message in &ir.messages {
        match message.role {
            IrRole::User => {
                let content: Vec<Value> = message
                    .parts
                    .iter()
                    .map(|part| match part {
                        IrPart::Text { text } => {
                            serde_json::json!({"type": "input_text", "text": text})
                        }

                        IrPart::Image { image } => serde_json::json!({
                            "type": "input_image",
                            "image_url": render_image_url(image),
                        }),

                        _ => Value::Null,
                    })
                    .collect();

                input.push(serde_json::json!({
                    "type": "message",
                    "role": "user",
                    "content": content,
                }));
            }

            IrRole::Assistant => {
                for part in &message.parts {
                    match part {
                        IrPart::Text { text } => {
                            input.push(serde_json::json!({
                                "type": "message",
                                "role": "assistant",
                                "content": [{"type": "output_text", "text": text}],
                            }));
                        }

                        IrPart::ToolCall {
                            call_id,
                            name,
                            arguments,
                            thought_signature,
                        } => {
                            let mut item = serde_json::json!({
                                "type": "function_call",
                                "call_id": call_id,
                                "name": name,
                                "arguments": arguments,
                            });

                            if let Some(signature) = thought_signature {
                                json_insert(
                                    &mut item,
                                    "thought_signature",
                                    Value::String(signature.clone()),
                                );
                            }

                            input.push(item);
                        }

                        _ => {}
                    }
                }
            }

            IrRole::Tool => {
                for part in &message.parts {
                    if let IrPart::ToolOutput { call_id, content } = part {
                        input.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": call_id,
                            "output": content,
                        }));
                    }
                }
            }
        }
    }

    let mut body = serde_json::json!({
        "model": ir.model,
        "input": input,
        "stream": stream,
        "store": false,
    });

    if let Some(instructions) = &ir.instructions {
        json_insert(
            &mut body,
            "instructions",
            Value::String(instructions.clone()),
        );
    }

    if !ir.tools.is_empty() {
        let tools: Vec<Value> = ir
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
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
            ToolChoice::Function { name } => serde_json::json!({"type": "function", "name": name}),
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
            "max_output_tokens",
            Value::from(max_output_tokens),
        );
    }

    if let Some(response_format) = &ir.response_format {
        let format = match response_format {
            IrResponseFormat::Text => serde_json::json!({"type": "text"}),
            IrResponseFormat::Json => serde_json::json!({"type": "json_object"}),
            IrResponseFormat::JsonSchema { name, schema } => {
                serde_json::json!({"type": "json_schema", "name": name, "schema": schema})
            }
        };

        json_insert(&mut body, "text", serde_json::json!({"format": format}));
    }

    if let Some(effort) = ir.reasoning_effort {
        json_insert(
            &mut body,
            "reasoning",
            serde_json::json!({"effort": effort.to_openai()}),
        );
    }

    body
}

/// Renders an [`IrImage`] back into a Responses image URL value.
fn render_image_url(image: &IrImage) -> String {
    match image {
        IrImage::Inline {
            mime_type,
            data_base64,
        } => format!("data:{mime_type};base64,{data_base64}"),

        IrImage::Url(url) => url.clone(),
    }
}

/// Function call being assembled from upstream events.
#[derive(Debug)]
struct PendingCall {
    /// Output index of the call item within the response.
    output_index: usize,

    /// Invocation identifier.
    call_id: String,
}

/// Parses upstream Responses payloads — SSE events or a full JSON body — into
/// IR events.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Function calls opened by `output_item.added` and awaiting completion.
    pending_calls: Vec<PendingCall>,
}

impl UpstreamParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ingests one upstream SSE frame carrying a `response.*` event.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for malformed JSON and upstream failure events.
    pub(crate) fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, ApiError> {
        let value: Value = serde_json::from_str(&frame.data)
            .map_err(|_| ApiError::upstream_unavailable("malformed JSON in upstream SSE frame"))?;

        self.ingest_event(value)
    }

    /// Parses a complete upstream non-streaming JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for upstream failure statuses and malformed
    /// bodies.
    pub(crate) fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, ApiError> {
        let mut events = self.message_start_events(&value);

        if let Some(output) = value.get("output").and_then(Value::as_array) {
            for item in output {
                self.ingest_output_item(item, None, &mut events)?;
            }
        }

        let status = value.get("status");

        if status.and_then(Value::as_str) == Some("failed") {
            let message = value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("upstream response failed");

            return Err(ApiError::upstream_unavailable(message));
        }

        events.push(IrEvent::Finish(finish_reason_for_non_failed_status(
            status.and_then(Value::as_str),
        )));

        if let Some(usage) = value.get("usage").filter(|usage| usage.is_object()) {
            events.push(IrEvent::Usage {
                input_tokens: usage
                    .get("input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                output_tokens: usage
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            });
        }

        Ok(events)
    }

    /// Ingests one upstream event object.
    fn ingest_event(&mut self, value: Value) -> Result<Vec<IrEvent>, ApiError> {
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match event_type {
            "response.created" | "response.in_progress" => Ok(self.message_start_events(&value)),

            "response.output_item.added" => {
                let mut events = self.message_start_events(&value);

                let output_index = event_output_index(&value);

                if let Some(item) = value.get("item")
                    && item.get("type").and_then(Value::as_str) == Some("function_call")
                {
                    events.push(self.open_function_call(item, output_index));
                }

                Ok(events)
            }

            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                Ok(self.delta_event(&value, IrEvent::ReasoningDelta))
            }

            "response.output_text.delta" | "response.refusal.delta" => {
                Ok(self.delta_event(&value, IrEvent::TextDelta))
            }

            "response.function_call_arguments.delta" => {
                let output_index = event_output_index(&value);

                let delta = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();

                Ok(vec![IrEvent::ToolCallDelta {
                    index: output_index,
                    call_id: None,
                    name: None,
                    arguments_delta: delta.to_string(),
                }])
            }

            "response.output_item.done" => {
                let mut events = Vec::new();

                let output_index = event_output_index(&value);

                if let Some(item) = value.get("item") {
                    self.ingest_output_item(item, Some(output_index), &mut events)?;
                }

                Ok(events)
            }

            "response.completed" => {
                let response = value.get("response").cloned().unwrap_or(Value::Null);

                let mut events = Vec::new();

                events.push(IrEvent::Finish(finish_reason_for_non_failed_status(
                    response.get("status").and_then(Value::as_str),
                )));

                if let Some(usage) = response.get("usage").filter(|usage| usage.is_object()) {
                    events.push(IrEvent::Usage {
                        input_tokens: usage
                            .get("input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        output_tokens: usage
                            .get("output_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                    });
                }

                Ok(events)
            }

            "response.failed" => {
                let message = value
                    .pointer("/response/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("upstream response failed");

                Err(ApiError::upstream_unavailable(message))
            }

            "error" => {
                let message = value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("upstream stream error");

                Err(ApiError::upstream_unavailable(message))
            }

            _ => Ok(Vec::new()),
        }
    }

    /// Ingests one output item from `output_item.done` or a JSON response.
    fn ingest_output_item(
        &mut self,
        item: &Value,
        output_index: Option<usize>,
        events: &mut Vec<IrEvent>,
    ) -> Result<(), ApiError> {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if part.get("type").and_then(Value::as_str) == Some("output_text")
                            && let Some(text) = part.get("text").and_then(Value::as_str)
                        {
                            events.push(IrEvent::TextDelta(text.to_string()));
                        }
                    }
                }

                Ok(())
            }

            Some("reasoning") => {
                if let Some(summary) = item.get("summary").and_then(Value::as_array) {
                    for part in summary {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            events.push(IrEvent::ReasoningDelta(text.to_string()));
                        }
                    }
                }

                Ok(())
            }

            Some("function_call") => {
                events.push(self.complete_function_call(item, output_index));

                Ok(())
            }

            _ => Ok(()),
        }
    }

    /// Emits the opening delta for a function call item revealed by
    /// `output_item.added`.
    fn open_function_call(&mut self, item: &Value, output_index: usize) -> IrEvent {
        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        self.pending_calls.push(PendingCall {
            output_index,
            call_id: call_id.clone(),
        });

        IrEvent::ToolCallDelta {
            index: output_index,
            call_id: Some(call_id),
            name: Some(name),
            arguments_delta: String::new(),
        }
    }

    /// Emits the completion of a function call item delivered by
    /// `output_item.done` or a JSON response.
    fn complete_function_call(&mut self, item: &Value, output_index: Option<usize>) -> IrEvent {
        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let resolved_index = output_index
            .or_else(|| {
                self.pending_calls
                    .iter()
                    .find(|pending| pending.call_id == call_id)
                    .map(|pending| pending.output_index)
            })
            .unwrap_or_default();

        self.pending_calls
            .retain(|pending| pending.call_id != call_id);

        IrEvent::ToolCallDone {
            index: resolved_index,
            call_id,
            name: item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            arguments: item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            thought_signature: item
                .get("thought_signature")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    }

    /// Builds a delta event from an upstream delta payload.
    fn delta_event(&self, value: &Value, make: impl Fn(String) -> IrEvent) -> Vec<IrEvent> {
        match value.get("delta") {
            Some(Value::String(delta)) => vec![make(delta.clone())],
            _ => Vec::new(),
        }
    }

    /// Emits [`IrEvent::MessageStart`] once per response.
    fn message_start_events(&mut self, value: &Value) -> Vec<IrEvent> {
        if self.is_started {
            return Vec::new();
        }

        self.is_started = true;

        let response = value
            .get("response")
            .cloned()
            .unwrap_or_else(|| value.clone());

        vec![IrEvent::MessageStart {
            id: response
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            model: response
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }]
    }
}

/// Reads the `output_index` field of an upstream stream event.
fn event_output_index(value: &Value) -> usize {
    value
        .get("output_index")
        .and_then(Value::as_u64)
        .unwrap_or_default() as usize
}

/// Finish reason for a status string already known not to be a failure.
fn finish_reason_for_non_failed_status(status: Option<&str>) -> FinishReason {
    if status == Some("incomplete") {
        FinishReason::Length
    } else {
        FinishReason::Stop
    }
}

/// One output item tracked by the client-side SSE renderer.
#[derive(Debug)]
struct OpenOutputItem {
    /// Position of the item within the output array.
    output_index: usize,

    /// The rendered item.
    item: Value,
}

/// Streaming client response renderer producing Responses SSE events.
#[derive(Debug)]
pub(crate) struct SseRenderer {
    /// Monotonic sequence number of the next event.
    sequence: u64,

    /// Gateway-issued response identifier.
    id: String,

    /// Public model name echoed to the client.
    model: String,

    /// Unix creation timestamp.
    created_at: i64,

    /// Whether `response.created` was emitted.
    is_created: bool,

    /// Open reasoning item awaiting completion: `(item_id, output_index,
    /// accumulated text)`.
    reasoning: Option<(String, usize, String)>,

    /// Open assistant message item awaiting completion.
    message: Option<(String, usize, String)>,

    /// Open function call items keyed by IR tool call index.
    calls: Vec<(usize, OpenOutputItem)>,

    /// Completed output items for the final response object.
    completed_items: Vec<OpenOutputItem>,

    /// Next output index to assign.
    next_output_index: usize,

    /// Token accounting observed before completion.
    usage: Option<(u64, u64)>,
}

impl SseRenderer {
    /// Creates a renderer for one streamed response.
    pub(crate) fn new(model: String) -> Self {
        Self {
            sequence: 0,
            id: format!("resp_{}", new_uuid()),
            model,
            created_at: unix_time::unix_now(),
            is_created: false,
            reasoning: None,
            message: None,
            calls: Vec::new(),
            completed_items: Vec::new(),
            next_output_index: 0,
            usage: None,
        }
    }

    /// Renders one IR event into zero or more SSE frames.
    pub(crate) fn render(&mut self, event: &IrEvent) -> Vec<String> {
        match event {
            IrEvent::MessageStart { .. } => {
                if self.is_created {
                    return Vec::new();
                }

                self.is_created = true;

                let shell = self.response_shell("in_progress", Value::Array(Vec::new()));

                vec![
                    self.event_frame(
                        "response.created",
                        serde_json::json!({"response": shell.clone()}),
                    ),
                    self.event_frame(
                        "response.in_progress",
                        serde_json::json!({"response": shell}),
                    ),
                ]
            }

            IrEvent::ReasoningDelta(delta) => {
                let (mut frames, item_id, output_index) = match &self.reasoning {
                    Some((item_id, output_index, _)) => {
                        (Vec::new(), item_id.clone(), *output_index)
                    }

                    None => {
                        let item_id = format!("rs_{}", new_uuid());
                        let output_index = self.next_output_index;

                        self.next_output_index += 1;
                        self.reasoning = Some((item_id.clone(), output_index, String::new()));

                        let mut frames = vec![self.event_frame(
                            "response.output_item.added",
                            serde_json::json!({
                                "output_index": output_index,
                                "item": {"id": item_id, "type": "reasoning", "summary": []},
                            }),
                        )];

                        frames.push(self.event_frame(
                            "response.reasoning_summary_part.added",
                            serde_json::json!({
                                "item_id": item_id,
                                "output_index": output_index,
                                "summary_index": 0,
                                "part": {"type": "summary_text", "text": ""},
                            }),
                        ));

                        (frames, item_id, output_index)
                    }
                };

                if let Some((_, _, accumulated)) = &mut self.reasoning {
                    accumulated.push_str(delta);
                }

                frames.push(self.event_frame(
                    "response.reasoning_summary_text.delta",
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "summary_index": 0,
                        "delta": delta,
                    }),
                ));

                frames
            }

            IrEvent::TextDelta(delta) => {
                let (mut frames, item_id, output_index) = match &self.message {
                    Some((item_id, output_index, _)) => {
                        (Vec::new(), item_id.clone(), *output_index)
                    }

                    None => {
                        let item_id = format!("msg_{}", new_uuid());
                        let output_index = self.next_output_index;

                        self.next_output_index += 1;
                        self.message = Some((item_id.clone(), output_index, String::new()));

                        let mut frames = vec![self.event_frame(
                            "response.output_item.added",
                            serde_json::json!({
                                "output_index": output_index,
                                "item": {
                                    "id": item_id,
                                    "type": "message",
                                    "role": "assistant",
                                    "status": "in_progress",
                                    "content": [],
                                },
                            }),
                        )];

                        frames.push(self.event_frame(
                            "response.content_part.added",
                            serde_json::json!({
                                "item_id": item_id,
                                "output_index": output_index,
                                "content_index": 0,
                                "part": {"type": "output_text", "text": "", "annotations": []},
                            }),
                        ));

                        (frames, item_id, output_index)
                    }
                };

                if let Some((_, _, accumulated)) = &mut self.message {
                    accumulated.push_str(delta);
                }

                frames.push(self.event_frame(
                    "response.output_text.delta",
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "delta": delta,
                    }),
                ));

                frames
            }

            IrEvent::ToolCallDelta {
                index,
                call_id,
                name,
                arguments_delta,
            } => {
                let mut frames = Vec::new();

                let (item_id, output_index) = match self
                    .calls
                    .iter()
                    .find(|(open_index, _)| open_index == index)
                {
                    Some((_, open)) => {
                        let item_id = open
                            .item
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();

                        (item_id, open.output_index)
                    }

                    None => {
                        let item_id = format!("fc_{}", new_uuid());
                        let output_index = self.next_output_index;

                        self.next_output_index += 1;

                        let mut item = serde_json::json!({
                            "id": item_id,
                            "type": "function_call",
                            "status": "in_progress",
                            "name": name,
                            "arguments": "",
                        });

                        if let Some(call_id) = call_id {
                            json_insert(&mut item, "call_id", Value::String(call_id.clone()));
                        }

                        frames.push(self.event_frame(
                            "response.output_item.added",
                            serde_json::json!({"output_index": output_index, "item": item.clone()}),
                        ));

                        self.calls
                            .push((*index, OpenOutputItem { output_index, item }));

                        (item_id, output_index)
                    }
                };

                if !arguments_delta.is_empty() {
                    frames.push(self.event_frame(
                        "response.function_call_arguments.delta",
                        serde_json::json!({
                            "item_id": item_id,
                            "output_index": output_index,
                            "delta": arguments_delta,
                        }),
                    ));
                }

                frames
            }

            IrEvent::ToolCallDone {
                index,
                call_id,
                name,
                arguments,
                thought_signature,
            } => {
                let Some(position) = self
                    .calls
                    .iter()
                    .position(|(open_index, _)| open_index == index)
                else {
                    return Vec::new();
                };

                let (_, mut open) = self.calls.swap_remove(position);

                json_insert(
                    &mut open.item,
                    "status",
                    Value::String("completed".to_string()),
                );
                json_insert(&mut open.item, "call_id", Value::String(call_id.clone()));
                json_insert(&mut open.item, "name", Value::String(name.clone()));
                json_insert(
                    &mut open.item,
                    "arguments",
                    Value::String(arguments.clone()),
                );

                if let Some(signature) = thought_signature {
                    json_insert(
                        &mut open.item,
                        "thought_signature",
                        Value::String(signature.clone()),
                    );
                }

                let output_index = open.output_index;
                let item_id = open
                    .item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();

                let mut frames = vec![self.event_frame(
                    "response.function_call_arguments.done",
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "arguments": arguments,
                    }),
                )];

                frames.push(self.event_frame(
                    "response.output_item.done",
                    serde_json::json!({"output_index": output_index, "item": open.item.clone()}),
                ));

                self.completed_items.push(OpenOutputItem {
                    output_index,
                    item: open.item,
                });

                frames
            }

            IrEvent::Finish(_) => self.close_open_items(),

            IrEvent::Usage {
                input_tokens,
                output_tokens,
            } => {
                self.usage = Some((*input_tokens, *output_tokens));

                Vec::new()
            }

            IrEvent::Done => {
                let mut frames = self.close_open_items();

                let mut output: Vec<(usize, Value)> = self
                    .completed_items
                    .iter()
                    .map(|open| (open.output_index, open.item.clone()))
                    .collect();

                output.sort_by_key(|(output_index, _)| *output_index);

                let output: Vec<Value> = output.into_iter().map(|(_, item)| item).collect();

                let (input_tokens, output_tokens) = self.usage.unwrap_or((0, 0));

                let mut shell = self.response_shell("completed", Value::Array(output));

                json_insert(
                    &mut shell,
                    "usage",
                    serde_json::json!({
                        "input_tokens": input_tokens,
                        "output_tokens": output_tokens,
                        "total_tokens": input_tokens + output_tokens,
                    }),
                );

                frames.push(
                    self.event_frame("response.completed", serde_json::json!({"response": shell})),
                );

                frames
            }
        }
    }

    /// Renders a mid-stream error as a terminal `response.failed` event.
    pub(crate) fn error_frame(&mut self, error: &ApiError) -> String {
        let payload = error.to_openai_json();

        let error_payload = payload
            .get("error")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        self.event_frame(
            "response.failed",
            serde_json::json!({
                "response": {
                    "id": self.id,
                    "object": "response",
                    "status": "failed",
                    "error": error_payload,
                }
            }),
        )
    }

    /// Closes any open reasoning or message items, emitting their done event
    /// sequences and recording the completed items.
    fn close_open_items(&mut self) -> Vec<String> {
        let mut frames = Vec::new();

        if let Some((item_id, output_index, text)) = self.reasoning.take() {
            for (event_name, payload) in [
                (
                    "response.reasoning_summary_text.done",
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "summary_index": 0,
                        "text": text,
                    }),
                ),
                (
                    "response.reasoning_summary_part.done",
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "summary_index": 0,
                        "part": {"type": "summary_text", "text": text},
                    }),
                ),
                (
                    "response.output_item.done",
                    serde_json::json!({
                        "output_index": output_index,
                        "item": {"id": item_id, "type": "reasoning", "summary": [
                            {"type": "summary_text", "text": text}
                        ]},
                    }),
                ),
            ] {
                frames.push(self.event_frame(event_name, payload));
            }

            self.completed_items.push(OpenOutputItem {
                output_index,
                item: serde_json::json!({
                    "id": item_id,
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": text}],
                }),
            });
        }

        if let Some((item_id, output_index, text)) = self.message.take() {
            let content_part =
                serde_json::json!({"type": "output_text", "text": text, "annotations": []});

            let completed_item = serde_json::json!({
                "id": item_id,
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [content_part.clone()],
            });

            for (event_name, payload) in [
                (
                    "response.output_text.done",
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "text": content_part.get("text").cloned().unwrap_or(Value::Null),
                    }),
                ),
                (
                    "response.content_part.done",
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "part": content_part,
                    }),
                ),
                (
                    "response.output_item.done",
                    serde_json::json!({
                        "output_index": output_index,
                        "item": completed_item.clone(),
                    }),
                ),
            ] {
                frames.push(self.event_frame(event_name, payload));
            }

            self.completed_items.push(OpenOutputItem {
                output_index,
                item: completed_item,
            });
        }

        frames
    }

    /// Builds the response object shell shared by all events.
    fn response_shell(&self, status: &str, output: Value) -> Value {
        serde_json::json!({
            "id": self.id,
            "object": "response",
            "created_at": self.created_at,
            "status": status,
            "model": self.model,
            "output": output,
        })
    }

    /// Formats one SSE frame with the given event name and payload, assigning
    /// the next sequence number.
    fn event_frame(&mut self, event_name: &str, mut payload: Value) -> String {
        json_insert(&mut payload, "sequence_number", Value::from(self.sequence));

        self.sequence += 1;

        format_frame(Some(event_name), &payload.to_string())
    }
}

/// Renders an aggregated completion as a non-streaming Responses API response.
pub(crate) fn render_json_response(model: &str, aggregation: &Aggregation) -> Value {
    let mut output: Vec<Value> = Vec::new();

    if !aggregation.reasoning.is_empty() {
        output.push(serde_json::json!({
            "id": format!("rs_{}", new_uuid()),
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": aggregation.reasoning}],
        }));
    }

    if !aggregation.text.is_empty() {
        output.push(serde_json::json!({
            "id": format!("msg_{}", new_uuid()),
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": aggregation.text, "annotations": []}],
        }));
    }

    for tool_call in &aggregation.tool_calls {
        let mut item = serde_json::json!({
            "id": format!("fc_{}", new_uuid()),
            "type": "function_call",
            "status": "completed",
            "call_id": tool_call.call_id,
            "name": tool_call.name,
            "arguments": tool_call.arguments,
        });

        if let Some(signature) = &tool_call.thought_signature {
            json_insert(
                &mut item,
                "thought_signature",
                Value::String(signature.clone()),
            );
        }

        output.push(item);
    }

    let (input_tokens, output_tokens) = aggregation.usage.unwrap_or((0, 0));

    serde_json::json!({
        "id": format!("resp_{}", new_uuid()),
        "object": "response",
        "created_at": unix_time::unix_now(),
        "status": "completed",
        "model": model,
        "output": output,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "total_tokens": input_tokens + output_tokens,
        },
    })
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "tests fail by panicking")]
#[expect(clippy::indexing_slicing, reason = "tests assert exact fixture shapes")]
mod tests {
    use super::{SseRenderer, UpstreamParser, parse_request, render_json_response};
    use crate::{
        ir::{Aggregator, FinishReason, IrEvent, IrPart},
        sse::SseParser,
    };
    use serde_json::Value;

    /// Drains raw SSE text through the upstream parser.
    ///
    /// # Panics
    ///
    /// Panics if frame parsing or ingestion fails.
    fn ingest_upstream(parser: &mut UpstreamParser, raw: &str) -> Vec<IrEvent> {
        let mut sse = SseParser::new();

        let mut events = Vec::new();

        for frame in sse.ingest(raw.as_bytes()).expect("frames should parse") {
            events.append(&mut parser.ingest_frame(&frame).expect("frame should ingest"));
        }

        events
    }

    /// Renders events through the client SSE renderer, returning parsed
    /// `(event name, payload)` pairs.
    ///
    /// # Panics
    ///
    /// Panics if a rendered frame is not well-formed.
    fn render_client(mut events: Vec<IrEvent>) -> Vec<(Option<String>, Value)> {
        let mut renderer = SseRenderer::new("public".to_string());

        let mut rendered = Vec::new();

        events.push(IrEvent::Done);

        for event in events {
            for frame in renderer.render(&event) {
                let (name, data) = frame
                    .strip_prefix("event: ")
                    .map(|rest| rest.split_once("\ndata: "))
                    .map(|found| {
                        let (name, data) = found.expect("event frame contains a data line");

                        (Some(name.to_string()), data.to_string())
                    })
                    .unwrap_or_else(|| {
                        (
                            None,
                            frame
                                .strip_prefix("data: ")
                                .expect("data frame prefix")
                                .to_string(),
                        )
                    });

                let payload = serde_json::from_str(data.trim_end()).expect("payload is JSON");

                rendered.push((name, payload));
            }
        }

        rendered
    }

    /// Verifies that parses input items into ir holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.

    #[test]
    fn parses_input_items_into_ir() {
        let parsed = parse_request(
            r#"{
                "model": "public",
                "instructions": "be nice",
                "input": [
                    {"type": "message", "role": "user", "content": "hello"},
                    {"type": "function_call", "call_id": "call_1", "name": "f",
                     "arguments": "{}", "thought_signature": "sig"},
                    {"type": "function_call_output", "call_id": "call_1", "output": "done"},
                    {"type": "reasoning", "summary": []}
                ],
                "tools": [{"type": "function", "name": "f"}],
                "reasoning": {"effort": "low"},
                "store": false
            }"#
            .as_bytes(),
        )
        .expect("request should parse");

        let ir = &parsed.ir;

        assert_eq!(ir.instructions.as_deref(), Some("be nice"));
        assert_eq!(ir.messages.len(), 3);
        assert_eq!(
            ir.messages[1].parts[0],
            IrPart::ToolCall {
                call_id: "call_1".to_string(),
                name: "f".to_string(),
                arguments: "{}".to_string(),
                thought_signature: Some("sig".to_string()),
            }
        );
        assert_eq!(ir.reasoning_effort, Some(crate::ir::ReasoningEffort::Low));
    }

    /// Verifies that rejects store true and unknown item fields holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.

    #[test]
    fn rejects_store_true_and_unknown_item_fields() {
        let store = parse_request(r#"{"model":"m","input":"x","store":true}"#.as_bytes())
            .expect_err("store true must fail");

        assert_eq!(store.status, 400);

        let unknown_field = super::validate_input_item_fields(Some(
            &serde_json::from_str::<Value>(
                r#"[{"type":"function_call_output","call_id":"c","output":"o","bogus":1}]"#,
            )
            .expect("json"),
        ))
        .expect_err("unknown field must fail");

        assert_eq!(unknown_field.param.as_deref(), Some("bogus"));
    }

    /// Verifies that upstream codex stream translates to ir events holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.

    #[test]
    fn upstream_codex_stream_translates_to_ir_events() {
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                concat!(
                    "data: {\"type\":\"response.created\",",
                    "\"response\":{\"id\":\"resp1\",\"model\":\"m\"}}\n\n"
                ),
                "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"hmm\"}\n\n",
                concat!(
                    "data: {\"type\":\"response.output_item.added\",\"output_index\":1,",
                    "\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",",
                    "\"name\":\"f\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.function_call_arguments.delta\",",
                    "\"output_index\":1,\"delta\":\"{}\"}\n\n"
                ),
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
                concat!(
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":1,",
                    "\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",",
                    "\"name\":\"f\",\"arguments\":\"{}\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.completed\",\"response\":",
                    "{\"status\":\"completed\",",
                    "\"usage\":{\"input_tokens\":4,\"output_tokens\":3}}}\n\n"
                ),
            ),
        );

        assert_eq!(
            events[0],
            IrEvent::MessageStart {
                id: "resp1".to_string(),
                model: "m".to_string(),
            }
        );

        assert!(events.contains(&IrEvent::ReasoningDelta("hmm".to_string())));
        assert!(events.contains(&IrEvent::TextDelta("hi".to_string())));
        assert!(events.contains(&IrEvent::ToolCallDelta {
            index: 1,
            call_id: Some("call_1".to_string()),
            name: Some("f".to_string()),
            arguments_delta: String::new(),
        }));
        assert!(events.contains(&IrEvent::ToolCallDone {
            index: 1,
            call_id: "call_1".to_string(),
            name: "f".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        }));
        assert!(events.contains(&IrEvent::Finish(FinishReason::Stop)));
        assert!(events.contains(&IrEvent::Usage {
            input_tokens: 4,
            output_tokens: 3,
        }));
    }

    /// Verifies that client stream renders conformant event sequence holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.

    #[test]
    fn client_stream_renders_conformant_event_sequence() {
        let events = vec![
            IrEvent::MessageStart {
                id: "up".to_string(),
                model: "m".to_string(),
            },
            IrEvent::ReasoningDelta("hmm".to_string()),
            IrEvent::TextDelta("hello".to_string()),
            IrEvent::Finish(FinishReason::Stop),
        ];

        let rendered = render_client(events);

        let names: Vec<&str> = rendered
            .iter()
            .map(|(name, _)| name.as_deref().unwrap_or("data"))
            .collect();

        assert_eq!(
            names,
            vec![
                "response.created",
                "response.in_progress",
                "response.output_item.added",
                "response.reasoning_summary_part.added",
                "response.reasoning_summary_text.delta",
                "response.output_item.added",
                "response.content_part.added",
                "response.output_text.delta",
                "response.reasoning_summary_text.done",
                "response.reasoning_summary_part.done",
                "response.output_item.done",
                "response.output_text.done",
                "response.content_part.done",
                "response.output_item.done",
                "response.completed",
            ]
        );

        let completed = rendered
            .iter()
            .find(|(name, _)| name.as_deref() == Some("response.completed"))
            .map(|(_, payload)| payload.clone())
            .expect("completed event");

        assert_eq!(completed["response"]["status"], "completed");
        assert_eq!(completed["response"]["usage"]["total_tokens"], 0);
        assert_eq!(
            completed["response"]["output"].as_array().map(Vec::len),
            Some(2)
        );
    }

    /// Verifies that aggregated response renders output items holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.

    #[test]
    fn aggregated_response_renders_output_items() {
        let mut aggregator = Aggregator::default();

        aggregator.ingest(&IrEvent::TextDelta("hey".to_string()));
        aggregator.ingest(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "f".to_string(),
            arguments: "{}".to_string(),
            thought_signature: Some("sig".to_string()),
        });
        aggregator.ingest(&IrEvent::Usage {
            input_tokens: 2,
            output_tokens: 1,
        });

        let rendered = render_json_response("public", &aggregator.finish());

        assert_eq!(rendered["status"], "completed");
        assert_eq!(rendered["output"].as_array().map(Vec::len), Some(2));
        assert_eq!(rendered["output"][1]["thought_signature"], "sig");
        assert_eq!(rendered["usage"]["total_tokens"], 3);
    }
}
