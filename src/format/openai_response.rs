//! OpenAI Responses protocol codec.
//!
//! Serves the outward `POST /responses` endpoint and speaks the
//! `openai_response` upstream protocol (including the Codex backend).

use super::{ClientFormat, ClientSseRenderer, ParsedRequest, UpstreamEventParser};
use crate::{
    error::{ApiError, invalid_request_from_json},
    format::{
        allowed_fields, arguments_as_object, json_insert, reject_unknown_fields,
        validate_content_part_fields,
    },
    ids::new_uuid,
    ir::{
        IrAggregation, IrEvent, IrFinishReason, IrImage, IrMessage, IrPart, IrReasoningEffort,
        IrRequest, IrResponseFormat, IrRole, IrTool, IrToolChoice, IrUsage,
    },
    sse::{SseFrame, format_frame},
    unix_time,
};
use serde::Deserialize;
use serde_json::Value;

/// Fields allowed on each Responses input item, keyed by item type. Unknown
/// fields are rejected because internally tagged serde enums cannot enforce
/// this.
///
/// `message` items accept and ignore `id` and `phase`: codex replays parsed
/// output items verbatim each turn, and the gateway itself stamps `id` values
/// onto rendered output items, so rejecting them would break history replay.
const INPUT_ITEM_FIELDS: &[(&str, &[&str])] = &[
    ("message", &["type", "id", "role", "content", "phase"]),
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
///
/// `output_text` accepts and ignores `annotations` and `logprobs`: the
/// gateway's own rendered `output_text` parts carry both fields, so
/// replaying an aggregated response verbatim must parse.
const CONTENT_PART_FIELDS: &[(&str, &[&str])] = &[
    ("input_text", &["type", "text"]),
    ("output_text", &["type", "text", "annotations", "logprobs"]),
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

/// Marker of the outward Responses client format.
#[derive(Debug)]
pub(crate) struct Format;

impl ClientFormat for Format {
    fn parse_request(body: &[u8]) -> Result<ParsedRequest, ApiError> {
        parse_request(body)
    }

    fn sse_renderer(
        _should_include_usage: bool,
        public_name: String,
    ) -> Box<dyn ClientSseRenderer + Send> {
        Box::new(SseRenderer::new(public_name))
    }

    fn render_json_response(public_name: &str, aggregation: &IrAggregation) -> Value {
        render_json_response(public_name, aggregation)
    }
}

/// Parses a Responses request body into IR.
///
/// # Errors
///
/// Returns [`ApiError`] for malformed JSON, unknown or unsupported fields,
/// invalid parameter ranges, and invalid image references.
fn parse_request(body: &[u8]) -> Result<ParsedRequest, ApiError> {
    let raw: Value = serde_json::from_slice(body).map_err(invalid_request_from_json)?;

    // Raw-field validation runs before deserialization so `raw` can be
    // consumed directly instead of cloned.
    validate_input_item_fields(raw.get("input"))?;

    let request: RequestBody = serde_json::from_value(raw).map_err(invalid_request_from_json)?;

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
                    instruction_parts.push(message_content_text(&content)?);
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

            ToolChoiceIn::Function { tool_type, name } => {
                if tool_type != "function" {
                    return Err(ApiError::invalid_parameter(
                        "type",
                        format!("Unsupported tool_choice type `{tool_type}`"),
                    ));
                }

                IrToolChoice::Function { name: name.clone() }
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
        Some(effort) => Some(IrReasoningEffort::parse_openai(effort).ok_or_else(|| {
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
        is_streaming: request.stream.unwrap_or(false),
        should_include_usage: false,
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

/// Extracts plain text from system/developer message content.
///
/// # Errors
///
/// Returns [`ApiError`] naming the offending content part when the message
/// carries non-text parts: instructions have no image channel, so silently
/// dropping parts would lose request content.
fn message_content_text(content: &MessageContentIn) -> Result<String, ApiError> {
    match content {
        MessageContentIn::Text(text) => Ok(text.clone()),

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

                    PartIn::InputImage { .. } => {
                        return Err(ApiError::invalid_parameter(
                            "content",
                            "System and developer message content parts must be text; \
                             `input_image` is not supported",
                        ));
                    }
                }
            }

            Ok(combined)
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
                        image: IrImage::from_url(image_url)?,
                    },
                });
            }

            Ok(converted)
        }
    }
}

/// Rejects unknown fields on Responses input items; internally tagged serde
/// enums cannot enforce this themselves.
///
/// Reasoning items are skipped wholesale: they are accepted and ignored, so
/// their payload is not validated.
///
/// # Errors
///
/// Returns [`ApiError`] naming the first unknown field.
fn validate_input_item_fields(input: Option<&Value>) -> Result<(), ApiError> {
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

        let Some(permitted_fields) = allowed_fields(INPUT_ITEM_FIELDS, item_type) else {
            continue;
        };

        reject_unknown_fields(
            object,
            permitted_fields,
            &format!("`{item_type}` input item"),
        )?;

        if let Some(parts) = object.get("content").and_then(Value::as_array) {
            validate_content_part_fields(parts, CONTENT_PART_FIELDS)?;
        }
    }

    Ok(())
}

/// Renders the Codex-flavored upstream Responses payload: streaming is
/// forced on (README §5.1 invariant 2) and `max_output_tokens` is omitted —
/// the ChatGPT Codex backend rejects it as an unsupported parameter, and the
/// model's own output bound applies instead of the client's cap.
///
/// # Panics
///
/// Panics if the rendered Responses body is not a JSON object, which
/// [`render_request`]'s construction makes impossible.
pub(crate) fn render_codex_request(ir: &IrRequest) -> Value {
    let mut body = render_request(ir, true);

    // render_request always yields a JSON object, so the removal lands.
    #[expect(
        clippy::expect_used,
        reason = "render_request always renders a JSON object body"
    )]
    let object = body
        .as_object_mut()
        .expect("the rendered Responses body should be a JSON object");

    object.remove("max_output_tokens");

    body
}

/// Renders an IR request into an upstream Responses payload.
pub(crate) fn render_request(ir: &IrRequest, is_streaming: bool) -> Value {
    let mut input: Vec<Value> = Vec::new();

    for message in &ir.messages {
        match message.role {
            IrRole::User => {
                let content: Vec<Value> = message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        IrPart::Text { text } => {
                            Some(serde_json::json!({"type": "input_text", "text": text}))
                        }

                        IrPart::Image { image } => Some(serde_json::json!({
                            "type": "input_image",
                            "image_url": image.to_url(),
                        })),

                        // The IR type system routes only text and images into
                        // user content.
                        _ => None,
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
        "stream": is_streaming,
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
            IrToolChoice::Auto => Value::String("auto".to_string()),
            IrToolChoice::None => Value::String("none".to_string()),
            IrToolChoice::Required => Value::String("required".to_string()),
            IrToolChoice::Function { name } => {
                serde_json::json!({"type": "function", "name": name})
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

/// Origin of an upstream output item, deciding how it maps to IR events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputItemOrigin {
    /// An item of a complete JSON response body; the value is the item's
    /// position in the `output` array.
    Json(usize),

    /// A streaming `output_item.done` event; the value is the upstream
    /// `output_index`.
    StreamDone(usize),

    /// An item of the terminal event's `response.output` array; the value is
    /// the item's position, used as the index of a call the terminal walk
    /// recovers. Consulted only for kinds that no live event delivered.
    Terminal(usize),
}

/// Parses upstream Responses payloads — SSE events or a full JSON body — into
/// IR events.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Whether live deltas already delivered the message text, marking done
    /// and terminal items as exhausted sources for it.
    is_text_delivered: bool,

    /// Whether live deltas already delivered the reasoning text, marking
    /// done and terminal items as exhausted sources for it.
    is_reasoning_delivered: bool,

    /// Whether a live event already delivered a tool call, marking the
    /// terminal output array as an exhausted source for them.
    is_call_delivered: bool,

    /// Whether a finish reason was already emitted; a clean end-of-stream
    /// without one degrades to [`IrFinishReason::Stop`] in the parser's
    /// finish hook, matching the aggregated rendering default.
    is_finished: bool,
}

impl UpstreamParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
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
                    self.is_call_delivered = true;

                    events.push(self.open_function_call(item, output_index));
                }

                Ok(events)
            }

            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let events = self.delta_event(&value, IrEvent::ReasoningDelta);

                // Only an event that actually carried a delta exhausts the
                // fallback sources; a delta-typed event without a payload
                // delivered nothing.
                if !events.is_empty() {
                    self.is_reasoning_delivered = true;
                }

                Ok(events)
            }

            "response.output_text.delta" | "response.refusal.delta" => {
                let events = self.delta_event(&value, IrEvent::TextDelta);

                if !events.is_empty() {
                    self.is_text_delivered = true;
                }

                Ok(events)
            }

            "response.function_call_arguments.delta" => {
                let output_index = event_output_index(&value);

                let delta = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();

                self.is_call_delivered = true;

                Ok(vec![IrEvent::ToolCallDelta {
                    index: output_index,
                    call_id: None,
                    name: None,
                    arguments_delta: delta.to_string(),
                }])
            }

            "response.output_item.done" => {
                let mut events = Vec::new();

                if let Some(item) = value.get("item") {
                    let output_index = event_output_index(&value);

                    self.ingest_output_item(
                        item,
                        OutputItemOrigin::StreamDone(output_index),
                        &mut events,
                    )?;
                }

                Ok(events)
            }

            "response.completed" | "response.incomplete" => {
                let response = value.get("response").cloned().unwrap_or(Value::Null);

                let mut events = Vec::new();

                // A third-party `openai_compatible` upstream may carry items
                // only in the terminal response object; when no live event
                // delivered a kind, recover it from the output array exactly
                // as from a JSON response body. The call recovery decision is
                // snapshotted before the walk: the walk must consult the
                // pre-walk state for every item, so a terminal output array
                // carrying several function_call items delivers each of them
                // instead of only the first.
                let is_call_recoverable = !self.is_call_delivered;

                // Whether the walk actually recovered calls: the delivery
                // flag keeps its pre-walk value for every item of the walk,
                // so the flip afterwards records the walk's outcome — the
                // finish mapping consults it, and a text-only output array
                // must not mark the turn as a tool-call turn.
                let mut is_call_recovered = false;

                if (!self.is_text_delivered || !self.is_reasoning_delivered || is_call_recoverable)
                    && let Some(output) = response.get("output").and_then(Value::as_array)
                {
                    for (position, item) in output.iter().enumerate() {
                        self.ingest_output_item(
                            item,
                            OutputItemOrigin::Terminal(position),
                            &mut events,
                        )?;

                        if is_call_recoverable
                            && item.get("type").and_then(Value::as_str) == Some("function_call")
                        {
                            is_call_recovered = true;
                        }
                    }
                }

                if is_call_recovered {
                    self.is_call_delivered = true;
                }

                self.is_finished = true;

                // The delivery flag was flipped after the terminal walk above,
                // so calls recovered from the output array count too.
                events.push(IrEvent::Finish(finish_reason_for_response(
                    &response,
                    self.is_call_delivered,
                )));
                push_usage_events(&response, &mut events);

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

    /// Ingests one output item from a JSON response body, a streaming
    /// `output_item.done` event, or the terminal event's output array.
    ///
    /// Conformant upstreams deliver message text, reasoning text, and tool
    /// calls as live events, so streamed and terminal items are fallback
    /// sources consulted only when no live event delivered that kind of item
    /// — third-party `openai_compatible` upstreams may stream complete items
    /// without deltas, and their items must still reach the client exactly
    /// once.
    fn ingest_output_item(
        &mut self,
        item: &Value,
        origin: OutputItemOrigin,
        events: &mut Vec<IrEvent>,
    ) -> Result<(), ApiError> {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                let should_extract = match origin {
                    OutputItemOrigin::Json(_) => true,

                    OutputItemOrigin::StreamDone(_) | OutputItemOrigin::Terminal(_) => {
                        !self.is_text_delivered
                    }
                };

                if should_extract
                    && let Some(content) = item.get("content").and_then(Value::as_array)
                {
                    for part in content {
                        // Refusal text is message text: it rides the same
                        // text channel as `output_text` parts (README §6.2).
                        let text = match part.get("type").and_then(Value::as_str) {
                            Some("output_text") => part.get("text").and_then(Value::as_str),

                            Some("refusal") => part.get("refusal").and_then(Value::as_str),

                            _ => None,
                        };

                        if let Some(text) = text {
                            // The delivery flag is set only when a delta was
                            // actually pushed, so a text-less item does not
                            // exhaust the source for a later item that
                            // carries the text.
                            self.is_text_delivered = true;

                            events.push(IrEvent::TextDelta(text.to_string()));
                        }
                    }
                }

                Ok(())
            }

            Some("reasoning") => {
                let should_extract = match origin {
                    OutputItemOrigin::Json(_) => true,

                    OutputItemOrigin::StreamDone(_) | OutputItemOrigin::Terminal(_) => {
                        !self.is_reasoning_delivered
                    }
                };

                if should_extract
                    && let Some(summary) = item.get("summary").and_then(Value::as_array)
                {
                    for part in summary {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            // Same per-extraction rule as the message arm: a
                            // summary-less item does not exhaust the source.
                            self.is_reasoning_delivered = true;

                            events.push(IrEvent::ReasoningDelta(text.to_string()));
                        }
                    }
                }

                Ok(())
            }

            Some("function_call") => {
                match origin {
                    // A JSON body delivers the whole call at once; mirror the
                    // chat and gemini JSON paths with a full-arguments delta
                    // plus a done per call, so streaming renderers open and
                    // complete the item.
                    OutputItemOrigin::Json(index) => {
                        self.is_call_delivered = true;

                        let call_id = string_field(item, "call_id");
                        let name = string_field(item, "name");
                        let arguments = string_field(item, "arguments");

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
                            thought_signature: item
                                .get("thought_signature")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        });
                    }

                    OutputItemOrigin::StreamDone(index) => {
                        self.is_call_delivered = true;

                        events.push(self.complete_function_call(item, index));
                    }

                    // The terminal fallback completes a call exactly-once:
                    // only when no live call event was delivered before the
                    // terminal event may the output array deliver calls,
                    // mirroring the per-kind text recovery; re-emitting
                    // delivered calls here would duplicate them. The delivery
                    // flag is owned by the terminal-event arm, which flips it
                    // only after the whole output array is walked, so several
                    // recovered calls each deliver exactly once; the flag
                    // keeps its pre-walk value for every item of this walk.
                    OutputItemOrigin::Terminal(index) => {
                        if !self.is_call_delivered {
                            events.push(self.complete_function_call(item, index));
                        }
                    }
                }

                Ok(())
            }

            _ => Ok(()),
        }
    }

    /// Emits the opening delta for a function call item revealed by
    /// `output_item.added`.
    fn open_function_call(&self, item: &Value, output_index: usize) -> IrEvent {
        IrEvent::ToolCallDelta {
            index: output_index,
            call_id: Some(string_field(item, "call_id")),
            name: Some(string_field(item, "name")),
            arguments_delta: String::new(),
        }
    }

    /// Emits the completion of a function call item delivered by a streaming
    /// `output_item.done`.
    fn complete_function_call(&self, item: &Value, index: usize) -> IrEvent {
        IrEvent::ToolCallDone {
            index,
            call_id: string_field(item, "call_id"),
            name: string_field(item, "name"),
            arguments: string_field(item, "arguments"),
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

impl UpstreamEventParser for UpstreamParser {
    /// Ingests one upstream SSE frame carrying a `response.*` event.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for malformed JSON and upstream failure events.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, ApiError> {
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
    fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, ApiError> {
        let mut events = self.message_start_events(&value);

        if let Some(output) = value.get("output").and_then(Value::as_array) {
            for (position, item) in output.iter().enumerate() {
                self.ingest_output_item(item, OutputItemOrigin::Json(position), &mut events)?;
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

        self.is_finished = true;

        // The output walk above flipped the delivery flag for every
        // `function_call` item the body carried.
        events.push(IrEvent::Finish(finish_reason_for_response(
            &value,
            self.is_call_delivered,
        )));

        push_usage_events(&value, &mut events);

        Ok(events)
    }

    fn finish(&mut self) -> Vec<IrEvent> {
        // An upstream may close its stream cleanly after streaming deltas
        // without ever sending the terminal `response.completed` or
        // `response.incomplete` event; the missing finish reason degrades to
        // `Stop` so the streamed client sees the same terminal finish as the
        // aggregated rendering of the same stream.
        if !self.is_finished {
            self.is_finished = true;

            return vec![IrEvent::Finish(IrFinishReason::Stop)];
        }

        Vec::new()
    }
}

/// Reads the `output_index` field of an upstream stream event.
fn event_output_index(value: &Value) -> usize {
    value
        .get("output_index")
        .and_then(Value::as_u64)
        .unwrap_or_default() as usize
}

/// Reads a string field of an upstream JSON object, defaulting to the empty
/// string.
fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Finish reason for a response object already known not to be failed.
///
/// An `incomplete` status maps through `incomplete_details.reason` (unknown
/// reasons degrade to [`IrFinishReason::Length`] — the output was cut short).
/// Any other status finished normally, and a normally finished turn that
/// delivered function calls carries the canonical [`IrFinishReason::ToolCall`]
/// finish: the Responses status vocabulary has no tool-call value (completed /
/// failed / in_progress / cancelled / queued / incomplete), so the delivered
/// calls decide, matching official Chat Completions semantics for
/// text+tool-call mixed turns too.
fn finish_reason_for_response(response: &Value, is_call_delivered: bool) -> IrFinishReason {
    if response.get("status").and_then(Value::as_str) != Some("incomplete") {
        if is_call_delivered {
            return IrFinishReason::ToolCall;
        }

        return IrFinishReason::Stop;
    }

    match response
        .pointer("/incomplete_details/reason")
        .and_then(Value::as_str)
    {
        Some("content_filter") => IrFinishReason::ContentFilter,

        _ => IrFinishReason::Length,
    }
}

/// Appends an [`IrEvent::Usage`] for the response's usage object, when present.
fn push_usage_events(response: &Value, events: &mut Vec<IrEvent>) {
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
}

/// Renders the `incomplete_details.reason` string for a finish reason that
/// marks an incomplete response; [`None`] for complete responses.
fn incomplete_details_reason(reason: IrFinishReason) -> Option<&'static str> {
    match reason {
        IrFinishReason::Length => Some("max_output_tokens"),

        IrFinishReason::ContentFilter => Some("content_filter"),

        IrFinishReason::Stop | IrFinishReason::ToolCall => None,
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
    usage: Option<IrUsage>,

    /// Finish reason reported before [`IrEvent::Done`], deciding whether the
    /// terminal event is `response.completed` or `response.incomplete`.
    finish: Option<IrFinishReason>,
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
            finish: None,
        }
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
            let content_part = serde_json::json!({
                "type": "output_text",
                "text": text,
                "annotations": [],
                "logprobs": [],
            });

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
                        "logprobs": [],
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
            // `parallel_tool_calls`, `tool_choice`, and `tools` are required
            // fields of the official Response shape (openai-python `Response`
            // types them without defaults), so the shell always carries the
            // statics describing the gateway itself: it advertises no
            // parallel tool-call control, tool calls happen whenever the
            // model decides (`auto`), and no tool definitions are echoed
            // back to the client.
            "parallel_tool_calls": false,
            "tool_choice": "auto",
            "tools": [],
        })
    }

    /// Formats one SSE frame with the given event name and payload, assigning
    /// the next sequence number.
    ///
    /// The payload carries `type` because strict clients — openai-python's
    /// `ResponseStreamEvent` union and openai/codex's serde type-tagged enum —
    /// discriminate events on the JSON payload, not the `event:` line.
    fn event_frame(&mut self, event_name: &str, mut payload: Value) -> String {
        json_insert(&mut payload, "type", Value::String(event_name.to_string()));

        json_insert(&mut payload, "sequence_number", Value::from(self.sequence));

        self.sequence += 1;

        format_frame(Some(event_name), &payload.to_string())
    }
}

impl ClientSseRenderer for SseRenderer {
    /// Renders one IR event into zero or more SSE frames.
    fn render(&mut self, event: &IrEvent) -> Vec<String> {
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
                                "part": {
                                    "type": "output_text",
                                    "text": "",
                                    "annotations": [],
                                    "logprobs": [],
                                },
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
                    // `logprobs` is a required field of the official event
                    // shape (openai-python `ResponseTextDeltaEvent`), so the
                    // gateway always carries it, empty when untracked.
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "delta": delta,
                        "logprobs": [],
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
                let position = self
                    .calls
                    .iter()
                    .position(|(open_index, _)| open_index == index);

                let (mut open, mut frames) = match position {
                    Some(position) => (self.calls.swap_remove(position).1, Vec::new()),

                    None => {
                        // A third-party upstream may deliver
                        // `output_item.done` for a call it never revealed
                        // with `output_item.added` or argument deltas — the
                        // recovery class of README §6.3. The done payload
                        // carries the complete call, so the item is opened
                        // here exactly as a live delta would have opened it;
                        // dropping it would lose the call only on the
                        // streaming path, while aggregation keeps it.
                        let item_id = format!("fc_{}", new_uuid());
                        let output_index = self.next_output_index;

                        self.next_output_index += 1;

                        let frames = vec![self.event_frame(
                            "response.output_item.added",
                            serde_json::json!({
                                "output_index": output_index,
                                "item": {
                                    "id": item_id,
                                    "type": "function_call",
                                    "status": "in_progress",
                                    "call_id": call_id,
                                    "name": name,
                                    "arguments": "",
                                },
                            }),
                        )];

                        (
                            OpenOutputItem {
                                output_index,
                                item: serde_json::json!({
                                    "id": item_id,
                                    "type": "function_call",
                                }),
                            },
                            frames,
                        )
                    }
                };

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

                frames.push(self.event_frame(
                    "response.function_call_arguments.done",
                    serde_json::json!({
                        "item_id": item_id,
                        "output_index": output_index,
                        "arguments": arguments,
                    }),
                ));

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

            IrEvent::Finish(reason) => {
                self.finish = Some(*reason);

                self.close_open_items()
            }

            IrEvent::Usage {
                input_tokens,
                output_tokens,
            } => {
                self.usage = Some(IrUsage {
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                });

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

                let usage = self.usage.unwrap_or_default();

                // A Length/ContentFilter finish must surface as
                // `response.incomplete` with `incomplete_details.reason`, so
                // strict clients can tell a truncated response from a full
                // one.
                let incomplete_reason = self.finish.and_then(incomplete_details_reason);
                let is_incomplete = incomplete_reason.is_some();

                let mut shell = self.response_shell(
                    if is_incomplete {
                        "incomplete"
                    } else {
                        "completed"
                    },
                    Value::Array(output),
                );

                json_insert(
                    &mut shell,
                    "usage",
                    serde_json::json!({
                        "input_tokens": usage.input_tokens,
                        "output_tokens": usage.output_tokens,
                        "total_tokens": usage.input_tokens + usage.output_tokens,
                    }),
                );

                if let Some(reason) = incomplete_reason {
                    json_insert(
                        &mut shell,
                        "incomplete_details",
                        serde_json::json!({"reason": reason}),
                    );
                }

                let terminal_event = if is_incomplete {
                    "response.incomplete"
                } else {
                    "response.completed"
                };

                frames
                    .push(self.event_frame(terminal_event, serde_json::json!({"response": shell})));

                frames
            }
        }
    }

    /// Renders a mid-stream error as a terminal `response.failed` event.
    fn error_frame(&mut self, error: &ApiError) -> String {
        let payload = error.to_openai_json();

        let error_payload = payload
            .get("error")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        // Strict clients validate the embedded response object like any
        // other, so it carries the full required shell rather than a bare
        // id/status/error subset.
        let mut shell = self.response_shell("failed", Value::Array(Vec::new()));

        json_insert(&mut shell, "error", error_payload);

        self.event_frame(
            "response.failed",
            serde_json::json!({
                "response": shell,
            }),
        )
    }
}

/// Renders an aggregated completion as a non-streaming Responses API response.
pub(crate) fn render_json_response(model: &str, aggregation: &IrAggregation) -> Value {
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
            "content": [{
                "type": "output_text",
                "text": aggregation.text,
                "annotations": [],
                "logprobs": [],
            }],
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

    let usage = aggregation.usage.unwrap_or_default();

    let incomplete_reason = aggregation.finish.and_then(incomplete_details_reason);

    let mut body = serde_json::json!({
        "id": format!("resp_{}", new_uuid()),
        "object": "response",
        "created_at": unix_time::unix_now(),
        "status": if incomplete_reason.is_some() {
            "incomplete"
        } else {
            "completed"
        },
        "model": model,
        "output": output,
        // Same required-field statics as the streaming shells: the gateway
        // advertises no parallel tool-call control, the model decides when
        // to call tools, and no tool definitions are echoed back.
        "parallel_tool_calls": false,
        "tool_choice": "auto",
        "tools": [],
        "usage": {
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "total_tokens": usage.input_tokens + usage.output_tokens,
        },
    });

    if let Some(reason) = incomplete_reason {
        json_insert(
            &mut body,
            "incomplete_details",
            serde_json::json!({"reason": reason}),
        );
    }

    body
}

#[cfg(test)]
mod tests {
    use super::{
        SseRenderer, UpstreamParser, parse_request, render_codex_request, render_json_response,
        render_request,
    };
    use crate::{
        error::ApiError,
        format::{
            ClientSseRenderer as _, UpstreamEventParser as _,
            openai_chat_completion::SseRenderer as ChatSseRenderer,
        },
        ir::{Aggregator, IrEvent, IrFinishReason, IrMessage, IrPart, IrRequest, IrRole, IrUsage},
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

        #[expect(
            clippy::expect_used,
            reason = "the streamed fixture text is well-formed SSE"
        )]
        for frame in sse.ingest(raw.as_bytes()).expect("frames should parse") {
            #[expect(
                clippy::expect_used,
                reason = "every fixture frame is a valid Responses payload"
            )]
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
                        #[expect(
                            clippy::expect_used,
                            reason = "an `event: ` frame was just matched with its data line"
                        )]
                        let (name, data) = found.expect("event frame contains a data line");

                        (Some(name.to_string()), data.to_string())
                    })
                    .unwrap_or_else(|| {
                        #[expect(
                            clippy::expect_used,
                            reason = "frames without an event name are bare data frames"
                        )]
                        let data = frame.strip_prefix("data: ").expect("data frame prefix");

                        (None, data.to_string())
                    });

                #[expect(clippy::expect_used, reason = "the renderer emits JSON payloads")]
                let payload = serde_json::from_str(data.trim_end()).expect("payload is JSON");

                rendered.push((name, payload));
            }
        }

        rendered
    }

    /// Parses Responses input items into IR.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn parses_input_items_into_ir() {
        #[expect(
            clippy::expect_used,
            reason = "the test body is a valid request fixture"
        )]
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

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so indexed messages are
        // read through annotated bindings first.
        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture parses exactly three messages, asserted above"
        )]
        let function_call_part = &ir.messages[1].parts[0];

        assert_eq!(
            function_call_part,
            &IrPart::ToolCall {
                call_id: "call_1".to_string(),
                name: "f".to_string(),
                arguments: "{}".to_string(),
                thought_signature: Some("sig".to_string()),
            }
        );
        assert_eq!(ir.reasoning_effort, Some(crate::ir::IrReasoningEffort::Low));
    }

    /// Rejects `store: true` and unknown input item fields.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn rejects_store_true_and_unknown_item_fields() {
        #[expect(clippy::expect_used, reason = "the test asserts this exact rejection")]
        let store = parse_request(r#"{"model":"m","input":"x","store":true}"#.as_bytes())
            .expect_err("store true must fail");

        assert_eq!(store.status, 400);

        #[expect(
            clippy::expect_used,
            reason = "the fixture is valid JSON and the test asserts this exact rejection"
        )]
        let unknown_field = super::validate_input_item_fields(Some(
            &serde_json::from_str::<Value>(
                r#"[{"type":"function_call_output","call_id":"c","output":"o","bogus":1}]"#,
            )
            .expect("json"),
        ))
        .expect_err("unknown field must fail");

        assert_eq!(unknown_field.param.as_deref(), Some("bogus"));
    }

    /// Translates a Codex upstream stream into IR events.
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

        // The turn mixed text with a delivered function call, so it carries
        // the canonical tool-call finish (README §6.3).
        assert!(events.contains(&IrEvent::Finish(IrFinishReason::ToolCall)));
        assert!(events.contains(&IrEvent::Usage {
            input_tokens: 4,
            output_tokens: 3,
        }));
    }

    /// Aggregates a Codex upstream stream whose `output_item.done` carries no
    /// assembled arguments, proving the streaming done path feeds the
    /// accumulated argument deltas into the aggregation.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn codex_stream_aggregates_tool_calls_from_done_event() {
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                concat!(
                    "data: {\"type\":\"response.output_item.added\",\"output_index\":0,",
                    "\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",",
                    "\"name\":\"f\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.function_call_arguments.delta\",",
                    "\"output_index\":0,\"delta\":\"{}\"}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,",
                    "\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",",
                    "\"name\":\"f\"}}\n\n"
                ),
            ),
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
                .map(|call| call.arguments.as_str()),
            Some("{}")
        );
    }

    /// Renders a conformant client streaming event sequence.
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
            IrEvent::Finish(IrFinishReason::Stop),
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

        #[expect(
            clippy::expect_used,
            reason = "the asserted sequence above renders a completed event"
        )]
        let completed = rendered
            .iter()
            .find(|(name, _)| name.as_deref() == Some("response.completed"))
            .map(|(_, payload)| payload.clone())
            .expect("completed event");

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so rendered fields are
        // read through annotated bindings first.
        #[expect(
            clippy::indexing_slicing,
            reason = "the completed payload always carries the response object"
        )]
        let response = &completed["response"];

        let status = &response["status"];

        #[expect(
            clippy::indexing_slicing,
            reason = "the completed payload always carries status and usage"
        )]
        let usage_total = &response["usage"]["total_tokens"];

        assert_eq!(status, "completed");
        assert_eq!(usage_total, 0);

        let output = &response["output"];

        assert_eq!(output.as_array().map(Vec::len), Some(2));
    }

    /// Carries the event name as the payload `type` on client streams.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn client_stream_payloads_carry_event_type() {
        let events = vec![
            IrEvent::MessageStart {
                id: "up".to_string(),
                model: "m".to_string(),
            },
            IrEvent::TextDelta("hello".to_string()),
            IrEvent::Finish(IrFinishReason::Stop),
        ];

        let rendered = render_client(events);

        assert!(!rendered.is_empty());

        for (name, payload) in &rendered {
            #[expect(
                clippy::expect_used,
                reason = "the rendered sequence asserted above names every event frame"
            )]
            let name = name
                .as_deref()
                .expect("every rendered responses frame has an event name");

            // Indexing through the borrowed payload does not trigger the
            // slicing lint; only the borrow itself needed an annotation.
            let payload_type = &payload["type"];

            assert_eq!(payload_type, name);
        }
    }

    /// Emits item text only via deltas on the upstream streaming path.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_stream_emits_item_text_only_via_deltas() {
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
                concat!(
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,",
                    "\"item\":{\"type\":\"message\",\"role\":\"assistant\",",
                    "\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}}\n\n"
                ),
                "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"hmm\"}\n\n",
                concat!(
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":1,",
                    "\"item\":{\"type\":\"reasoning\",\"summary\":[",
                    "{\"type\":\"summary_text\",\"text\":\"hmm\"}]}}\n\n"
                ),
            ),
        );

        let text_deltas: usize = events
            .iter()
            .filter_map(|event| match event {
                IrEvent::TextDelta(text) if text == "hi" => Some(()),
                _ => None,
            })
            .count();

        assert_eq!(text_deltas, 1);

        let reasoning_deltas: usize = events
            .iter()
            .filter_map(|event| match event {
                IrEvent::ReasoningDelta(text) if text == "hmm" => Some(()),
                _ => None,
            })
            .count();

        assert_eq!(reasoning_deltas, 1);
    }

    /// Recovers message text from the `output_item.done` events of a
    /// third-party upstream that streams complete items but no text deltas;
    /// the terminal output array carries the same item and must not repeat
    /// it, so the text stays exactly-once.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_done_only_stream_extracts_item_text_once() {
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                concat!(
                    "data: {\"type\":\"response.created\",",
                    "\"response\":{\"id\":\"resp1\",\"model\":\"m\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.output_item.added\",\"output_index\":0,",
                    "\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,",
                    "\"item\":{\"type\":\"message\",\"role\":\"assistant\",",
                    "\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.completed\",\"response\":{\"status\":",
                    "\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",",
                    "\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}],",
                    "\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n\n"
                ),
            ),
        );

        let mut aggregator = Aggregator::default();

        for event in &events {
            aggregator.ingest(event);
        }

        let aggregation = aggregator.finish();

        assert_eq!(aggregation.text, "hi");
        assert_eq!(
            aggregation.usage,
            Some(IrUsage {
                input_tokens: 1,
                output_tokens: 2,
            })
        );
    }

    /// Renders a tool call delivered only by `output_item.done` — no
    /// `output_item.added` or argument deltas preceded it — into both client
    /// streams, matching the aggregated result, which already keeps
    /// standalone done events (README §6.3 recovers this upstream class).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_done_only_tool_call_reaches_both_client_streams() {
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                concat!(
                    "data: {\"type\":\"response.created\",",
                    "\"response\":{\"id\":\"resp1\",\"model\":\"m\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,",
                    "\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",",
                    "\"name\":\"f\",\"arguments\":\"{}\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.completed\",",
                    "\"response\":{\"status\":\"completed\"}}\n\n"
                ),
            ),
        );

        assert!(events.contains(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "f".to_string(),
            arguments: "{}".to_string(),
            thought_signature: None,
        }));

        // The upstream opened no item and streamed no deltas, so the parser
        // emitted a standalone done.
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, IrEvent::ToolCallDelta { .. }))
        );

        // Outward Responses: the call is opened and completed as an output
        // item and lands in the terminal response's output array.
        let rendered = render_client(events.clone());

        #[expect(
            clippy::expect_used,
            reason = "the fixture asserts a rendered output_item.done event"
        )]
        let item_done = rendered
            .iter()
            .find(|(name, _)| name.as_deref() == Some("response.output_item.done"))
            .map(|(_, payload)| payload.clone())
            .expect("output_item.done event");

        let item = item_done.get("item").cloned().unwrap_or_default();

        assert_eq!(
            item.get("type").and_then(Value::as_str),
            Some("function_call")
        );
        assert_eq!(
            item.get("status").and_then(Value::as_str),
            Some("completed")
        );
        assert_eq!(item.get("call_id").and_then(Value::as_str), Some("call_1"));
        assert_eq!(item.get("name").and_then(Value::as_str), Some("f"));
        assert_eq!(item.get("arguments").and_then(Value::as_str), Some("{}"));

        #[expect(
            clippy::expect_used,
            reason = "the asserted sequence above renders a completed event"
        )]
        let completed = rendered
            .iter()
            .find(|(name, _)| name.as_deref() == Some("response.completed"))
            .map(|(_, payload)| payload.clone())
            .expect("completed event");

        let completed_item = completed
            .pointer("/response/output/0")
            .cloned()
            .unwrap_or_default();

        assert_eq!(
            completed_item.get("type").and_then(Value::as_str),
            Some("function_call")
        );

        assert_eq!(
            completed_item.get("call_id").and_then(Value::as_str),
            Some("call_1")
        );

        assert_eq!(
            completed_item.get("arguments").and_then(Value::as_str),
            Some("{}")
        );

        // Outward Chat Completions: the never-opened done streams as one
        // full tool_calls delta chunk.
        let mut chat_renderer = ChatSseRenderer::new("public".to_string(), false);

        let mut chat_tool_calls: Vec<Value> = Vec::new();

        for event in &events {
            for frame in chat_renderer.render(event) {
                #[expect(
                    clippy::expect_used,
                    reason = "the renderer emits `data: ` prefixed JSON frames"
                )]
                let payload: Value = serde_json::from_str(
                    frame
                        .strip_prefix("data: ")
                        .expect("data frame prefix")
                        .trim_end(),
                )
                .expect("payload is JSON");

                if let Some(tool_calls) = payload
                    .pointer("/choices/0/delta/tool_calls")
                    .and_then(Value::as_array)
                {
                    chat_tool_calls.extend(tool_calls.iter().cloned());
                }
            }
        }

        assert_eq!(chat_tool_calls.len(), 1);

        #[expect(
            clippy::expect_used,
            reason = "the fixture asserts exactly one streamed tool call exists"
        )]
        let chat_tool_call = chat_tool_calls.first().expect("one streamed tool call");

        assert_eq!(chat_tool_call.get("index").and_then(Value::as_u64), Some(0));
        assert_eq!(
            chat_tool_call.get("id").and_then(Value::as_str),
            Some("call_1")
        );
        assert_eq!(
            chat_tool_call.get("type").and_then(Value::as_str),
            Some("function")
        );
        assert_eq!(
            chat_tool_call
                .pointer("/function/name")
                .and_then(Value::as_str),
            Some("f")
        );
        assert_eq!(
            chat_tool_call
                .pointer("/function/arguments")
                .and_then(Value::as_str),
            Some("{}")
        );

        // Aggregated: the standalone done is kept, so both streams must
        // deliver the same call.
        let mut aggregator = Aggregator::default();

        for event in &events {
            aggregator.ingest(event);
        }

        let aggregation = aggregator.finish();

        assert_eq!(aggregation.tool_calls.len(), 1);

        assert_eq!(
            aggregation.tool_calls.first().map(|call| (
                call.call_id.as_str(),
                call.name.as_str(),
                call.arguments.as_str()
            )),
            Some(("call_1", "f", "{}"))
        );
    }

    /// Recovers reasoning and message text from the output array of a
    /// third-party upstream that emits only the terminal
    /// `response.completed` event.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_completed_only_stream_extracts_output_text() {
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                concat!(
                    "data: {\"type\":\"response.created\",",
                    "\"response\":{\"id\":\"resp1\",\"model\":\"m\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp1\",",
                    "\"model\":\"m\",\"status\":\"completed\",\"output\":[",
                    "{\"type\":\"reasoning\",\"summary\":[",
                    "{\"type\":\"summary_text\",\"text\":\"hmm\"}]},",
                    "{\"type\":\"message\",\"role\":\"assistant\",\"content\":[",
                    "{\"type\":\"output_text\",\"text\":\"hi\"}]}],",
                    "\"usage\":{\"input_tokens\":4,\"output_tokens\":3}}}\n\n"
                ),
            ),
        );

        let mut aggregator = Aggregator::default();

        for event in &events {
            aggregator.ingest(event);
        }

        let aggregation = aggregator.finish();

        assert_eq!(aggregation.text, "hi");
        assert_eq!(aggregation.reasoning, "hmm");
        assert_eq!(aggregation.finish, Some(IrFinishReason::Stop));
        assert_eq!(
            aggregation.usage,
            Some(IrUsage {
                input_tokens: 4,
                output_tokens: 3,
            })
        );
    }

    /// Extracts message text and reasoning from an upstream JSON response.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_json_response_extracts_message_and_reasoning() {
        let mut parser = UpstreamParser::new();

        #[expect(clippy::expect_used, reason = "the fixture is a valid Responses body")]
        let events = parser
            .parse_json_response(serde_json::json!({
                "id": "resp1",
                "model": "m",
                "status": "completed",
                "output": [
                    {"type": "reasoning",
                     "summary": [{"type": "summary_text", "text": "hmm"}]},
                    {"type": "message", "role": "assistant",
                     "content": [{"type": "output_text", "text": "hi"}]},
                ],
                "usage": {"input_tokens": 2, "output_tokens": 3},
            }))
            .expect("response should parse");

        assert!(events.contains(&IrEvent::ReasoningDelta("hmm".to_string())));
        assert!(events.contains(&IrEvent::TextDelta("hi".to_string())));
        assert!(events.contains(&IrEvent::Finish(IrFinishReason::Stop)));
        assert!(events.contains(&IrEvent::Usage {
            input_tokens: 2,
            output_tokens: 3,
        }));
    }

    /// Yields a finish reason and usage from `response.incomplete` events.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_incomplete_event_yields_finish_and_usage() {
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                concat!(
                    "data: {\"type\":\"response.created\",",
                    "\"response\":{\"id\":\"resp1\",\"model\":\"m\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.incomplete\",",
                    "\"response\":{\"status\":\"incomplete\",",
                    "\"incomplete_details\":{\"reason\":\"max_output_tokens\"},",
                    "\"usage\":{\"input_tokens\":4,\"output_tokens\":3}}}\n\n"
                ),
            ),
        );

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::Length)));
        assert!(events.contains(&IrEvent::Usage {
            input_tokens: 4,
            output_tokens: 3,
        }));

        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                concat!(
                    "data: {\"type\":\"response.created\",",
                    "\"response\":{\"id\":\"resp1\",\"model\":\"m\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.incomplete\",",
                    "\"response\":{\"status\":\"incomplete\",",
                    "\"incomplete_details\":{\"reason\":\"content_filter\"}}}\n\n"
                ),
            ),
        );

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::ContentFilter)));
    }

    /// Renders `response.incomplete` as the terminal client event for truncated
    /// responses.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn client_stream_renders_incomplete_terminal_event() {
        for (reason, expected) in [
            (IrFinishReason::Length, "max_output_tokens"),
            (IrFinishReason::ContentFilter, "content_filter"),
        ] {
            let events = vec![
                IrEvent::MessageStart {
                    id: "up".to_string(),
                    model: "m".to_string(),
                },
                IrEvent::TextDelta("partial".to_string()),
                IrEvent::Finish(reason),
            ];

            let rendered = render_client(events);

            #[expect(
                clippy::expect_used,
                reason = "the asserted sequence above renders an incomplete terminal event"
            )]
            let terminal = rendered
                .iter()
                .find(|(name, _)| name.as_deref() == Some("response.incomplete"))
                .expect("incomplete terminal event");

            // Attributes on macro-invocation statements do not cover
            // desugared index expressions inside the expansion, so the
            // terminal fields are read through annotated bindings first.
            #[expect(
                clippy::indexing_slicing,
                reason = "the incomplete payload always carries the response object"
            )]
            let response = &terminal.1["response"];

            let status = &response["status"];

            #[expect(
                clippy::indexing_slicing,
                reason = "the incomplete payload always carries status and details"
            )]
            let reason_field = &response["incomplete_details"]["reason"];

            assert_eq!(status, "incomplete");
            assert_eq!(reason_field, expected);

            assert!(
                rendered
                    .iter()
                    .all(|(name, _)| name.as_deref() != Some("response.completed"))
            );
        }
    }

    /// Renders an aggregated incomplete response with `incomplete` status.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn aggregated_incomplete_response_renders_incomplete_status() {
        let mut aggregator = Aggregator::default();

        aggregator.ingest(&IrEvent::TextDelta("partial".to_string()));
        aggregator.ingest(&IrEvent::Finish(IrFinishReason::ContentFilter));

        let rendered = render_json_response("public", &aggregator.finish());

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so rendered fields are
        // read through annotated bindings first.
        #[expect(
            clippy::indexing_slicing,
            reason = "render_json_response always emits the status and details"
        )]
        let status = &rendered["status"];

        #[expect(
            clippy::indexing_slicing,
            reason = "render_json_response always emits the status and details"
        )]
        let reason_field = &rendered["incomplete_details"]["reason"];

        assert_eq!(status, "incomplete");
        assert_eq!(reason_field, "content_filter");
    }

    /// Rejects image parts in system messages.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn rejects_image_parts_in_system_messages() {
        #[expect(clippy::expect_used, reason = "the test asserts this exact rejection")]
        let error = parse_request(
            r#"{
                "model": "public",
                "input": [
                    {"type": "message", "role": "system", "content": [
                        {"type": "input_text", "text": "be nice"},
                        {"type": "input_image", "image_url": "data:image/png;base64,QUJD"}
                    ]}
                ]
            }"#
            .as_bytes(),
        )
        .expect_err("image parts in system messages must fail");

        assert_eq!(error.status, 400);
        assert_eq!(error.param.as_deref(), Some("content"));
    }

    /// Renders aggregated output items.
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

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so rendered fields are
        // read through annotated bindings first.
        #[expect(
            clippy::indexing_slicing,
            reason = "render_json_response always emits the status and output array"
        )]
        let status = &rendered["status"];

        #[expect(
            clippy::indexing_slicing,
            reason = "render_json_response always emits the status and output array"
        )]
        let output = &rendered["output"];

        let second_item = &output[1];

        #[expect(
            clippy::indexing_slicing,
            reason = "render_json_response always emits the usage object"
        )]
        let total_tokens = &rendered["usage"]["total_tokens"];

        let second_item_signature = &second_item["thought_signature"];

        assert_eq!(status, "completed");
        assert_eq!(output.as_array().map(Vec::len), Some(2));
        assert_eq!(second_item_signature, "sig");
        assert_eq!(total_tokens, 3);
    }

    /// Accepts and ignores the optional `id` and `phase` fields that codex
    /// replays on `message` input items, so replaying the gateway's own
    /// rendered output items is not rejected as an unknown field.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn accepts_id_and_phase_on_message_input_items() {
        #[expect(
            clippy::expect_used,
            reason = "the fixture is a valid request; the test asserts acceptance"
        )]
        let parsed = parse_request(
            r#"{
                "model": "public",
                "input": [
                    {"type": "message", "id": "msg_1", "role": "assistant",
                     "content": [{"type": "output_text", "text": "hi"}],
                     "phase": "final_answer"}
                ]
            }"#
            .as_bytes(),
        )
        .expect("message item id and phase must be accepted and ignored");

        assert_eq!(parsed.ir.messages.len(), 1);

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so the indexed message is
        // read through an annotated binding first.
        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture parses exactly one message, asserted above"
        )]
        let first_message = &parsed.ir.messages[0];

        assert_eq!(
            first_message.parts,
            vec![IrPart::Text {
                text: "hi".to_string()
            }]
        );
    }

    /// Translates a JSON (non-SSE) upstream response carrying parallel tool
    /// calls and proves the calls survive into both outward streaming formats
    /// with collision-free indices, instead of being silently dropped.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_json_tool_calls_render_into_both_client_formats() {
        let mut parser = UpstreamParser::new();

        #[expect(clippy::expect_used, reason = "the fixture is a valid Responses body")]
        let events = parser
            .parse_json_response(serde_json::json!({
                "id": "resp1",
                "model": "m",
                "status": "completed",
                "output": [
                    {"type": "function_call", "call_id": "call_1",
                     "name": "weather", "arguments": "{\"city\":\"oslo\"}"},
                    {"type": "function_call", "call_id": "call_2",
                     "name": "time", "arguments": "{\"tz\":\"utc\"}"},
                ],
            }))
            .expect("response should parse");

        let done_indices: Vec<usize> = events
            .iter()
            .filter_map(|event| match event {
                IrEvent::ToolCallDone { index, .. } => Some(*index),
                _ => None,
            })
            .collect();

        assert_eq!(done_indices, vec![0, 1]);

        // Outward Responses: both calls appear as completed output items.
        let rendered = render_client(events.clone());

        #[expect(
            clippy::expect_used,
            reason = "the asserted sequence above renders a completed event"
        )]
        let completed = rendered
            .iter()
            .find(|(name, _)| name.as_deref() == Some("response.completed"))
            .map(|(_, payload)| payload.clone())
            .expect("completed event");

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so the output array is
        // read through an annotated binding first.
        #[expect(
            clippy::indexing_slicing,
            reason = "the completed payload always carries the response output array"
        )]
        #[expect(
            clippy::expect_used,
            reason = "the fixture asserts a completed response carrying output items"
        )]
        let output = completed["response"]["output"]
            .as_array()
            .expect("completed response output array");

        assert_eq!(output.len(), 2);

        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture carries exactly two function calls, asserted above"
        )]
        let first_item = &output[0];

        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture carries exactly two function calls, asserted above"
        )]
        let second_item = &output[1];

        // Indexing through the borrowed items does not trigger the slicing
        // lint; the item extraction above carries the annotation.
        let first_type = &first_item["type"];

        let first_call_id = &first_item["call_id"];

        let first_arguments = &first_item["arguments"];

        let second_call_id = &second_item["call_id"];

        let second_arguments = &second_item["arguments"];

        assert_eq!(first_type, "function_call");
        assert_eq!(first_call_id, "call_1");
        assert_eq!(first_arguments, "{\"city\":\"oslo\"}");
        assert_eq!(second_call_id, "call_2");
        assert_eq!(second_arguments, "{\"tz\":\"utc\"}");

        // Outward Chat Completions: both calls stream as tool-call deltas.
        let mut chat_renderer = ChatSseRenderer::new("public".to_string(), false);

        let mut chat_names: Vec<Value> = Vec::new();

        for event in &events {
            for frame in chat_renderer.render(event) {
                #[expect(
                    clippy::expect_used,
                    reason = "the renderer emits `data: ` prefixed JSON frames"
                )]
                let payload: Value = serde_json::from_str(
                    frame
                        .strip_prefix("data: ")
                        .expect("data frame prefix")
                        .trim_end(),
                )
                .expect("payload is JSON");

                // Attributes on macro-invocation statements do not cover
                // desugared index expressions inside the expansion, so the
                // choices entry is read through an annotated binding first.
                #[expect(
                    clippy::indexing_slicing,
                    reason = "chat chunks always carry a single choices entry"
                )]
                let tool_calls = payload["choices"][0]["delta"]["tool_calls"]
                    .as_array()
                    .into_iter()
                    .flatten();

                for tool_call in tool_calls {
                    #[expect(
                        clippy::indexing_slicing,
                        reason = "tool-call deltas always carry their function object"
                    )]
                    let name = &tool_call["function"]["name"];

                    chat_names.push(name.clone());
                }
            }
        }

        assert_eq!(chat_names, vec!["weather", "time"]);
    }

    /// Emits the `logprobs` field that openai-python requires on
    /// `response.output_text.delta` and `.done` events and carries on the
    /// `output_text` content part.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn output_text_events_and_parts_carry_logprobs() {
        let events = vec![
            IrEvent::MessageStart {
                id: "up".to_string(),
                model: "m".to_string(),
            },
            IrEvent::TextDelta("hi".to_string()),
            IrEvent::Finish(IrFinishReason::Stop),
        ];

        let rendered = render_client(events);

        for event_name in ["response.output_text.delta", "response.output_text.done"] {
            #[expect(
                clippy::expect_used,
                reason = "the rendered sequence asserted above includes the text events"
            )]
            let payload = rendered
                .iter()
                .find(|(name, _)| name.as_deref() == Some(event_name))
                .map(|(_, payload)| payload)
                .expect("output text event must be rendered");

            // Indexing through the borrowed payload does not trigger the
            // slicing lint; only the find above needed an annotation.
            let logprobs = &payload["logprobs"];

            assert_eq!(logprobs, &Value::Array(Vec::new()));
        }

        for event_name in ["response.content_part.added", "response.content_part.done"] {
            #[expect(
                clippy::expect_used,
                reason = "the rendered sequence asserted above includes the part events"
            )]
            let payload = rendered
                .iter()
                .find(|(name, _)| name.as_deref() == Some(event_name))
                .map(|(_, payload)| payload)
                .expect("content part event must be rendered");

            #[expect(
                clippy::indexing_slicing,
                reason = "content parts always carry the logprobs field"
            )]
            let logprobs = &payload["part"]["logprobs"];

            assert_eq!(logprobs, &Value::Array(Vec::new()));
        }

        let mut aggregator = Aggregator::default();

        aggregator.ingest(&IrEvent::TextDelta("hi".to_string()));

        let body = render_json_response("public", &aggregator.finish());

        #[expect(
            clippy::indexing_slicing,
            reason = "render_json_response emits one output item with one text part"
        )]
        let output_logprobs = &body["output"][0]["content"][0]["logprobs"];

        assert_eq!(output_logprobs, &Value::Array(Vec::new()));
    }

    /// Asserts that a Response object carries the fields the official
    /// openai-python `Response` type requires without defaults: the tool
    /// statics describing what the gateway supports.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    fn assert_required_response_fields(response: &Value) {
        assert_eq!(
            response.get("parallel_tool_calls"),
            Some(&Value::Bool(false))
        );

        assert_eq!(
            response.get("tool_choice"),
            Some(&Value::String("auto".to_string()))
        );

        assert_eq!(response.get("tools"), Some(&Value::Array(Vec::new())));
    }

    /// Carries `parallel_tool_calls`, `tool_choice`, and `tools` — required
    /// fields of the official Response shape — on every rendered response
    /// object: the streaming shells, the terminal body, the aggregated
    /// non-streaming body, and the `response.failed` frame.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn response_objects_carry_required_fields() {
        let events = vec![
            IrEvent::MessageStart {
                id: "up".to_string(),
                model: "m".to_string(),
            },
            IrEvent::TextDelta("hello".to_string()),
            IrEvent::Finish(IrFinishReason::Stop),
        ];

        let rendered = render_client(events);

        for event_name in [
            "response.created",
            "response.in_progress",
            "response.completed",
        ] {
            #[expect(
                clippy::expect_used,
                reason = "the asserted sequence above renders each shell event"
            )]
            let shell = rendered
                .iter()
                .find(|(name, _)| name.as_deref() == Some(event_name))
                .and_then(|(_, payload)| payload.get("response"))
                .expect("shell event must carry the response object");

            assert_required_response_fields(shell);
        }

        let body = render_json_response("public", &Aggregator::default().finish());

        assert_required_response_fields(&body);

        let mut renderer = SseRenderer::new("public".to_string());

        let frame = renderer.error_frame(&ApiError::internal("boom"));

        #[expect(
            clippy::expect_used,
            reason = "the error frame is a well-formed `response.failed` SSE frame"
        )]
        let payload: Value = serde_json::from_str(
            frame
                .strip_prefix("event: response.failed\ndata: ")
                .expect("failed frame prefix")
                .trim_end(),
        )
        .expect("payload is JSON");

        let failed_response = payload.get("response").cloned().unwrap_or_default();

        assert_eq!(
            failed_response.get("status").and_then(Value::as_str),
            Some("failed")
        );

        assert_eq!(
            failed_response.get("model").and_then(Value::as_str),
            Some("public")
        );

        assert!(failed_response.get("created_at").is_some_and(Value::is_i64));

        assert_eq!(
            failed_response.get("output"),
            Some(&Value::Array(Vec::new()))
        );

        assert_eq!(
            failed_response
                .pointer("/error/message")
                .and_then(Value::as_str),
            Some("boom")
        );

        assert_required_response_fields(&failed_response);
    }

    /// Degrades a clean end-of-stream that carried no terminal
    /// `response.completed`/`response.incomplete` event to a single
    /// `Finish(Stop)`, matching the aggregated rendering of the same stream,
    /// whose outward body defaults the status to `completed`; the streamed
    /// client then also sees a terminal finish.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn clean_eof_without_terminal_event_yields_finish_stop() {
        let mut parser = UpstreamParser::new();

        let mut events = ingest_upstream(
            &mut parser,
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
        );

        assert!(!events.contains(&IrEvent::Finish(IrFinishReason::Stop)));

        events.append(&mut parser.finish());

        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, IrEvent::Finish(_)))
                .count(),
            1
        );

        assert_eq!(events.last(), Some(&IrEvent::Finish(IrFinishReason::Stop)));
    }

    /// Never duplicates the finish reason: a clean end-of-stream after a
    /// terminal `response.completed` event yields exactly one finish event.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn clean_eof_after_terminal_event_does_not_duplicate_finish() {
        let mut parser = UpstreamParser::new();

        let mut events = ingest_upstream(
            &mut parser,
            concat!(
                "data: {\"type\":\"response.completed\",",
                "\"response\":{\"status\":\"completed\"}}\n\n"
            ),
        );

        events.append(&mut parser.finish());

        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, IrEvent::Finish(_)))
                .count(),
            1
        );
    }

    /// Recovers a function call from the output array of a third-party
    /// upstream that emits only the terminal `response.completed` event and
    /// no live call events: the call reaches the aggregation and both client
    /// streams exactly once (README §6.3 recovery class).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_completed_only_stream_recovers_function_calls() {
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                concat!(
                    "data: {\"type\":\"response.created\",",
                    "\"response\":{\"id\":\"resp1\",\"model\":\"m\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp1\",",
                    "\"model\":\"m\",\"status\":\"completed\",\"output\":[",
                    "{\"type\":\"function_call\",\"call_id\":\"call_1\",",
                    "\"name\":\"f\",\"arguments\":\"{}\"}]}}\n\n"
                ),
            ),
        );

        // The terminal walk completed the call exactly once.
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, IrEvent::ToolCallDone { .. }))
                .count(),
            1
        );

        // Aggregated: the recovered call is kept.
        let mut aggregator = Aggregator::default();

        for event in &events {
            aggregator.ingest(event);
        }

        let aggregation = aggregator.finish();

        assert_eq!(
            aggregation.tool_calls.first().map(|call| (
                call.call_id.as_str(),
                call.name.as_str(),
                call.arguments.as_str()
            )),
            Some(("call_1", "f", "{}"))
        );

        // Outward Responses: the call is opened and completed as an output
        // item and lands in the terminal response's output array.
        let rendered = render_client(events.clone());

        #[expect(
            clippy::expect_used,
            reason = "the fixture asserts a rendered output_item.done event"
        )]
        let item_done = rendered
            .iter()
            .find(|(name, _)| name.as_deref() == Some("response.output_item.done"))
            .map(|(_, payload)| payload.clone())
            .expect("output_item.done event");

        let item = item_done.get("item").cloned().unwrap_or_default();

        assert_eq!(
            item.get("type").and_then(Value::as_str),
            Some("function_call")
        );

        assert_eq!(item.get("call_id").and_then(Value::as_str), Some("call_1"));

        // Outward Chat Completions: the call streams as one full tool_calls
        // delta chunk.
        let mut chat_renderer = ChatSseRenderer::new("public".to_string(), false);

        let mut chat_tool_calls: Vec<Value> = Vec::new();

        for event in &events {
            for frame in chat_renderer.render(event) {
                #[expect(
                    clippy::expect_used,
                    reason = "the renderer emits `data: ` prefixed JSON frames"
                )]
                let payload: Value = serde_json::from_str(
                    frame
                        .strip_prefix("data: ")
                        .expect("data frame prefix")
                        .trim_end(),
                )
                .expect("payload is JSON");

                if let Some(tool_calls) = payload
                    .pointer("/choices/0/delta/tool_calls")
                    .and_then(Value::as_array)
                {
                    chat_tool_calls.extend(tool_calls.iter().cloned());
                }
            }
        }

        assert_eq!(chat_tool_calls.len(), 1);
    }

    /// Recovers every function call from the output array of a third-party
    /// upstream that emits only the terminal `response.completed` event and
    /// no live call events: a terminal output carrying two calls delivers
    /// each of them to the aggregation and both client streams exactly once
    /// (README §6.3 recovery class).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_completed_only_stream_recovers_every_function_call() {
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                concat!(
                    "data: {\"type\":\"response.created\",",
                    "\"response\":{\"id\":\"resp1\",\"model\":\"m\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp1\",",
                    "\"model\":\"m\",\"status\":\"completed\",\"output\":[",
                    "{\"type\":\"function_call\",\"call_id\":\"call_1\",",
                    "\"name\":\"f\",\"arguments\":\"{}\"},",
                    "{\"type\":\"function_call\",\"call_id\":\"call_2\",",
                    "\"name\":\"g\",\"arguments\":\"{\\\"x\\\":1}\"}]}}\n\n"
                ),
            ),
        );

        // The terminal walk completed both calls exactly once.
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, IrEvent::ToolCallDone { .. }))
                .count(),
            2
        );

        // Aggregated: both recovered calls are kept, in output order.
        let mut aggregator = Aggregator::default();

        for event in &events {
            aggregator.ingest(event);
        }

        let aggregation = aggregator.finish();

        assert_eq!(
            aggregation
                .tool_calls
                .iter()
                .map(|call| (
                    call.call_id.as_str(),
                    call.name.as_str(),
                    call.arguments.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![("call_1", "f", "{}"), ("call_2", "g", "{\"x\":1}")]
        );

        // Outward Responses: each recovered call is completed as its own
        // function_call output item.
        let rendered = render_client(events.clone());

        let completed_items = rendered
            .iter()
            .filter(|(name, _)| name.as_deref() == Some("response.output_item.done"))
            .map(|(_, payload)| payload.get("item").cloned().unwrap_or_default())
            .collect::<Vec<_>>();

        assert_eq!(
            completed_items
                .iter()
                .map(|item| item.get("call_id").and_then(Value::as_str))
                .collect::<Vec<_>>(),
            vec![Some("call_1"), Some("call_2")]
        );

        // Outward Chat Completions: each call streams as one full tool_calls
        // delta chunk.
        let mut chat_renderer = ChatSseRenderer::new("public".to_string(), false);

        let mut chat_tool_calls: Vec<Value> = Vec::new();

        for event in &events {
            for frame in chat_renderer.render(event) {
                #[expect(
                    clippy::expect_used,
                    reason = "the renderer emits `data: ` prefixed JSON frames"
                )]
                let payload: Value = serde_json::from_str(
                    frame
                        .strip_prefix("data: ")
                        .expect("data frame prefix")
                        .trim_end(),
                )
                .expect("payload is JSON");

                if let Some(tool_calls) = payload
                    .pointer("/choices/0/delta/tool_calls")
                    .and_then(Value::as_array)
                {
                    chat_tool_calls.extend(tool_calls.iter().cloned());
                }
            }
        }

        assert_eq!(chat_tool_calls.len(), 2);
    }

    /// Never duplicates calls the stream already completed live: a terminal
    /// output array repeating a delivered function call is skipped, so the
    /// call reaches the aggregation exactly once.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_completed_output_array_does_not_duplicate_delivered_calls() {
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                concat!(
                    "data: {\"type\":\"response.output_item.added\",\"output_index\":0,",
                    "\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",",
                    "\"name\":\"f\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.function_call_arguments.delta\",",
                    "\"output_index\":0,\"delta\":\"{}\"}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,",
                    "\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",",
                    "\"name\":\"f\",\"arguments\":\"{}\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.completed\",\"response\":{\"status\":",
                    "\"completed\",\"output\":[{\"type\":\"function_call\",",
                    "\"call_id\":\"call_1\",\"name\":\"f\",\"arguments\":\"{}\"}]}}\n\n"
                ),
            ),
        );

        let mut aggregator = Aggregator::default();

        for event in &events {
            aggregator.ingest(event);
        }

        let aggregation = aggregator.finish();

        assert_eq!(aggregation.tool_calls.len(), 1);

        assert_eq!(
            aggregation
                .tool_calls
                .first()
                .map(|call| call.call_id.as_str()),
            Some("call_1")
        );
    }

    /// Recovers the text of a later message item when an earlier done item of
    /// a done-only upstream carried none: only an item that actually
    /// extracts text exhausts the per-kind recovery source.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_done_only_stream_extracts_text_from_later_items() {
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                concat!(
                    "data: {\"type\":\"response.created\",",
                    "\"response\":{\"id\":\"resp1\",\"model\":\"m\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,",
                    "\"item\":{\"type\":\"message\",\"role\":\"assistant\",",
                    "\"content\":[]}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":1,",
                    "\"item\":{\"type\":\"message\",\"role\":\"assistant\",",
                    "\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}}\n\n"
                ),
            ),
        );

        let mut aggregator = Aggregator::default();

        for event in &events {
            aggregator.ingest(event);
        }

        let aggregation = aggregator.finish();

        assert_eq!(aggregation.text, "hi");
    }

    /// Finishes a tool-call turn with the canonical `ToolCall` reason on both
    /// upstream shapes: the streaming terminal event and the JSON body. The
    /// Responses status vocabulary has no tool-call value (openai-python
    /// `ResponseStatus`: completed/failed/in_progress/cancelled/queued/
    /// incomplete), so a successfully completed turn that delivered function
    /// calls is the `ToolCall` finish, matching official Chat semantics; a
    /// text-only turn keeps finishing `Stop` (README §6.3).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_tool_call_turn_finishes_with_tool_call() {
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                concat!(
                    "data: {\"type\":\"response.created\",",
                    "\"response\":{\"id\":\"resp1\",\"model\":\"m\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.output_item.added\",\"output_index\":0,",
                    "\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",",
                    "\"name\":\"f\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.function_call_arguments.delta\",",
                    "\"output_index\":0,\"delta\":\"{}\"}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,",
                    "\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",",
                    "\"name\":\"f\",\"arguments\":\"{}\"}}\n\n"
                ),
                concat!(
                    "data: {\"type\":\"response.completed\",",
                    "\"response\":{\"status\":\"completed\"}}\n\n"
                ),
            ),
        );

        assert_eq!(
            events.last(),
            Some(&IrEvent::Finish(IrFinishReason::ToolCall))
        );

        // The JSON body shape agrees.
        let mut parser = UpstreamParser::new();

        #[expect(clippy::expect_used, reason = "the fixture is a valid Responses body")]
        let events = parser
            .parse_json_response(serde_json::json!({
                "id": "resp1",
                "model": "m",
                "status": "completed",
                "output": [
                    {"type": "function_call", "call_id": "call_1",
                     "name": "f", "arguments": "{}"},
                ],
            }))
            .expect("response should parse");

        assert_eq!(
            events.last(),
            Some(&IrEvent::Finish(IrFinishReason::ToolCall))
        );

        // A text-only turn keeps finishing `Stop`.
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
                concat!(
                    "data: {\"type\":\"response.completed\",",
                    "\"response\":{\"status\":\"completed\"}}\n\n"
                ),
            ),
        );

        assert_eq!(events.last(), Some(&IrEvent::Finish(IrFinishReason::Stop)));

        let mut parser = UpstreamParser::new();

        #[expect(clippy::expect_used, reason = "the fixture is a valid Responses body")]
        let events = parser
            .parse_json_response(serde_json::json!({
                "id": "resp1",
                "model": "m",
                "status": "completed",
                "output": [
                    {"type": "message", "role": "assistant",
                     "content": [{"type": "output_text", "text": "hi"}]},
                ],
            }))
            .expect("response should parse");

        assert_eq!(events.last(), Some(&IrEvent::Finish(IrFinishReason::Stop)));
    }

    /// Delivers upstream refusal text through the ordinary message text
    /// channel: the `response.refusal.delta` streaming event (pinned) and
    /// `refusal` content parts of JSON and terminal message items, exactly
    /// like `output_text` parts (README §6.2).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_refusal_delivers_as_message_text() {
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            "data: {\"type\":\"response.refusal.delta\",\"delta\":\"cannot\"}\n\n",
        );

        assert!(events.contains(&IrEvent::TextDelta("cannot".to_string())));

        // The JSON body path extracts `refusal` content parts as message text.
        let mut parser = UpstreamParser::new();

        #[expect(clippy::expect_used, reason = "the fixture is a valid Responses body")]
        let events = parser
            .parse_json_response(serde_json::json!({
                "id": "resp1",
                "model": "m",
                "status": "completed",
                "output": [
                    {"type": "message", "role": "assistant",
                     "content": [{"type": "refusal", "refusal": "cannot help"}]},
                ],
            }))
            .expect("response should parse");

        assert!(events.contains(&IrEvent::TextDelta("cannot help".to_string())));

        // The terminal-recovery walk treats refusal parts as message text
        // exactly like output_text parts: a live refusal delta mutes the
        // terminal array's refusal part, keeping the text exactly-once.
        let mut parser = UpstreamParser::new();

        let events = ingest_upstream(
            &mut parser,
            concat!(
                concat!(
                    "data: {\"type\":\"response.created\",",
                    "\"response\":{\"id\":\"resp1\",\"model\":\"m\"}}\n\n"
                ),
                "data: {\"type\":\"response.refusal.delta\",\"delta\":\"cannot\"}\n\n",
                concat!(
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp1\",",
                    "\"status\":\"completed\",\"output\":[",
                    "{\"type\":\"message\",\"role\":\"assistant\",\"content\":[",
                    "{\"type\":\"refusal\",\"refusal\":\"cannot\"}]}]}}\n\n"
                ),
            ),
        );

        assert_eq!(
            events
                .iter()
                .filter(|event| event == &&IrEvent::TextDelta("cannot".to_string()))
                .count(),
            1
        );
    }

    /// Accepts and ignores `annotations` and `logprobs` on `output_text`
    /// input content parts: the gateway's own rendered `output_text` parts
    /// carry both fields (openai-python types `annotations` without a
    /// default), so replaying an aggregated response verbatim must parse
    /// (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn accepts_annotations_and_logprobs_on_output_text_parts() {
        #[expect(
            clippy::expect_used,
            reason = "the fixture is a valid request; the test asserts acceptance"
        )]
        let parsed = parse_request(
            r#"{
                "model": "public",
                "input": [
                    {"type": "message", "role": "assistant", "content": [
                        {"type": "output_text", "text": "hi",
                         "annotations": [], "logprobs": []}
                    ]}
                ]
            }"#
            .as_bytes(),
        )
        .expect("output_text annotations and logprobs must be accepted and ignored");

        // Attributes on macro-invocation statements do not cover desugared
        // index expressions inside the expansion, so the indexed message is
        // read through an annotated binding first.
        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture parses exactly one message, asserted below"
        )]
        let first_message = &parsed.ir.messages[0];

        assert_eq!(
            first_message.parts,
            vec![IrPart::Text {
                text: "hi".to_string()
            }]
        );
    }

    /// Builds a minimal IR request carrying one user text turn.
    fn user_turn_request() -> IrRequest {
        IrRequest {
            model: "up-model".to_string(),
            messages: vec![IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: "hi".to_string(),
                }],
            }],
            ..IrRequest::default()
        }
    }

    /// Verifies the upstream Responses payload always carries `store: false`
    /// and renders the requested streaming mode verbatim (README §5.1
    /// invariant 1): the gateway is stateless, whatever the client asks.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn render_request_always_stores_false_and_renders_stream_mode() {
        let ir = user_turn_request();

        for stream in [true, false] {
            let body = render_request(&ir, stream);

            assert_eq!(body.get("store"), Some(&Value::Bool(false)));

            assert_eq!(body.get("stream"), Some(&Value::Bool(stream)));
        }
    }

    /// Verifies the Codex-flavored render forces streaming upstream and drops
    /// `max_output_tokens`, which the ChatGPT Codex backend rejects as an
    /// unsupported parameter — the model's own output bound applies (README
    /// §5.1) — while the shared compatible render keeps the client's cap
    /// (README §5.3).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn render_codex_request_forces_streaming_and_drops_max_output_tokens() {
        let ir = IrRequest {
            max_output_tokens: Some(16),
            ..user_turn_request()
        };

        let body = render_codex_request(&ir);

        assert_eq!(body.get("stream"), Some(&Value::Bool(true)));

        assert_eq!(body.get("store"), Some(&Value::Bool(false)));

        assert_eq!(body.get("max_output_tokens"), None);

        let compatible = render_request(&ir, true);

        assert_eq!(compatible.get("max_output_tokens"), Some(&Value::from(16)));
    }

    /// Verifies `reasoning.summary` strictness: only `"auto"` is accepted;
    /// any other value is rejected with 400 naming `summary` (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn rejects_unsupported_reasoning_summary_and_accepts_auto() {
        #[expect(clippy::expect_used, reason = "the test asserts this exact rejection")]
        let error =
            parse_request(br#"{"model":"m","input":"x","reasoning":{"summary":"minimal"}}"#)
                .expect_err("an unsupported reasoning summary must fail");

        assert_eq!(error.status, 400);

        assert_eq!(error.param.as_deref(), Some("summary"));

        #[expect(
            clippy::expect_used,
            reason = "the summary auto fixture is a valid request"
        )]
        let parsed = parse_request(br#"{"model":"m","input":"x","reasoning":{"summary":"auto"}}"#)
            .expect("reasoning summary auto must be accepted");

        assert_eq!(parsed.ir.model, "m");
    }
}
