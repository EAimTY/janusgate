//! Client-request parsing for the Gemini GenerateContent codec: the inbound GenerateContent body
//! deserialized into the IR request.

use crate::{
    format::{
        ParsedRequest, RouteContext,
        gemini::{GeminiResponseFormatRejection, parse_gemini_response_format},
    },
    ir::{
        InvalidRequest, IrImage, IrMessage, IrPart, IrRequest, IrResponseFormat, IrRole, IrTool,
        IrToolChoice,
    },
    vocabulary::ReasoningLevel,
};
use serde::{Deserialize, de::IgnoredAny};
use serde_json::{Map, Value};

/// Strictly parsed GenerateContent request body.
///
/// The model rides the URL path and streaming rides the URL action, so neither appears here:
/// a body carrying `model` or `stream` fails as an unknown field, enforcing the route-context
/// contract. Every field accepts its official camelCase alias alongside the snake_case name.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestBody {
    /// System instructions.
    #[serde(default, alias = "systemInstruction")]
    system_instruction: Option<SystemInstructionIn>,

    /// Conversation contents.
    contents: Vec<ContentIn>,

    /// Advertised function tools.
    #[serde(default)]
    tools: Option<Vec<ToolIn>>,

    /// Tool selection restriction.
    #[serde(default, alias = "toolConfig")]
    tool_config: Option<ToolConfigIn>,

    /// Sampling and output configuration.
    #[serde(default, alias = "generationConfig")]
    generation_config: Option<GenerationConfigIn>,

    /// Known-but-unsupported stop controls: each present value is a 400 naming the field.
    #[serde(default)]
    stop: Option<IgnoredAny>,

    /// Known-but-unsupported stop sequences.
    #[serde(default, alias = "stopSequences")]
    stop_sequences: Option<IgnoredAny>,

    /// Known-but-unsupported seed.
    #[serde(default)]
    seed: Option<IgnoredAny>,

    /// Known-but-unsupported speech configuration.
    #[serde(default, alias = "speechConfig")]
    speech_config: Option<IgnoredAny>,

    /// Known-but-unsupported audio configuration.
    #[serde(default, alias = "audioConfig")]
    audio_config: Option<IgnoredAny>,

    /// Known-but-unsupported video configuration.
    #[serde(default, alias = "videoConfig")]
    video_config: Option<IgnoredAny>,
}

/// System instructions: text parts only.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemInstructionIn {
    /// Instruction parts.
    parts: Vec<SystemPartIn>,
}

/// One system instruction part: plain text only.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemPartIn {
    /// Instruction text.
    text: String,
}

/// One conversation content: the parts of a single role turn.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContentIn {
    /// Content role: `user` or `model`.
    role: String,

    /// Ordered parts.
    parts: Vec<PartIn>,
}

/// One content part: exactly one payload member, with the gateway-emitted companions
/// (`thoughtSignature` beside `functionCall`, `id` inside calls/responses, `thought` beside
/// text) accepted for replay.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartIn {
    /// Plain text.
    #[serde(default)]
    text: Option<String>,

    /// Reasoning marker the gateway itself emits on thought parts; accepted-and-ignored,
    /// mapping to plain text (this surface never originates reasoning blocks).
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
    thought: Option<bool>,

    /// Inline image data.
    #[serde(default, alias = "inlineData")]
    inline_data: Option<InlineDataIn>,

    /// Remote image reference.
    #[serde(default, alias = "fileData")]
    file_data: Option<FileDataIn>,

    /// Assistant tool invocation.
    #[serde(default, alias = "functionCall")]
    function_call: Option<FunctionCallIn>,

    /// Tool result.
    #[serde(default, alias = "functionResponse")]
    function_response: Option<FunctionResponseIn>,

    /// Upstream thought signature replayed across turns; valid only beside `functionCall`.
    #[serde(default, alias = "thoughtSignature")]
    thought_signature: Option<String>,
}

/// Inline image data.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InlineDataIn {
    /// MIME type of the image data.
    #[serde(alias = "mimeType")]
    mime_type: String,

    /// Base64-encoded image bytes.
    data: String,
}

/// Remote image reference.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDataIn {
    /// Image URL.
    #[serde(alias = "fileUri")]
    file_uri: String,

    /// Declared MIME type; accepted-and-ignored (the IR URL image carries no MIME).
    #[serde(default, alias = "mimeType")]
    #[expect(
        dead_code,
        reason = "accepted and ignored; IR URL images carry no MIME"
    )]
    mime_type: Option<String>,
}

/// Assistant tool invocation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FunctionCallIn {
    /// Invoked function name.
    name: String,

    /// Arguments object; absent means no arguments.
    #[serde(default, alias = "arguments")]
    args: Option<Value>,

    /// Invocation identifier the gateway emits for replay pairing.
    #[serde(default)]
    id: Option<String>,
}

/// Tool result.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FunctionResponseIn {
    /// Answered function name.
    name: String,

    /// Result payload: an object flattens to JSON text, a string rides verbatim.
    response: Value,

    /// Invocation identifier the gateway emits for replay pairing.
    #[serde(default)]
    id: Option<String>,

    /// The official media channel on tool results, known-but-unsupported: presence is a
    /// 400 naming the field (the IR tool output has no media channel;
    /// `docs/reference/inference.md`).
    #[serde(default)]
    parts: Option<IgnoredAny>,

    /// Non-blocking scheduling control for resumable calls, accepted-and-ignored: the
    /// stateless gateway schedules nothing downstream (`docs/reference/inference.md`).
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
    scheduling: Option<String>,

    /// Continuation marker for multi-part results, accepted-and-ignored: the stateless
    /// gateway never continues a tool result across turns (`docs/reference/inference.md`).
    #[serde(default, alias = "willContinue")]
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
    will_continue: Option<bool>,
}

/// One advertised tool group: function declarations only.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolIn {
    /// Function declarations of the group.
    #[serde(alias = "functionDeclarations")]
    function_declarations: Option<Vec<FunctionDeclarationIn>>,
}

/// One advertised function declaration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FunctionDeclarationIn {
    /// Function name.
    name: String,

    /// Human-readable description.
    #[serde(default)]
    description: Option<String>,

    /// JSON Schema of accepted arguments.
    #[serde(default)]
    parameters: Option<Value>,
}

/// Tool selection configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolConfigIn {
    /// Function calling configuration.
    #[serde(alias = "functionCallingConfig")]
    function_calling_config: FunctionCallingConfigIn,
}

/// Function calling configuration: AUTO/ANY/NONE plus an optional single allowed function.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FunctionCallingConfigIn {
    /// Selection mode: `AUTO`, `ANY`, or `NONE`.
    mode: String,

    /// Functions the model may call under `ANY`.
    #[serde(default, alias = "allowedFunctionNames")]
    allowed_function_names: Option<Vec<String>>,
}

/// Sampling and output configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationConfigIn {
    /// Sampling temperature.
    #[serde(default)]
    temperature: Option<f64>,

    /// Nucleus sampling threshold.
    #[serde(default, alias = "topP")]
    top_p: Option<f64>,

    /// Known-but-unsupported nucleus truncation; present is a 400 naming the field.
    #[serde(default, alias = "topK")]
    top_k: Option<IgnoredAny>,

    /// Output token budget.
    #[serde(default, alias = "maxOutputTokens")]
    max_output_tokens: Option<u64>,

    /// Known-but-unsupported stop sequences; present is a 400 naming the field.
    #[serde(default, alias = "stopSequences")]
    stop_sequences: Option<IgnoredAny>,

    /// Known-but-unsupported stop control; present is a 400 naming the field.
    #[serde(default)]
    stop: Option<IgnoredAny>,

    /// Known-but-unsupported seed; present is a 400 naming the field.
    #[serde(default)]
    seed: Option<IgnoredAny>,

    /// Direct thinking level (alias for the `thinkingConfig` slot).
    #[serde(default, alias = "thinkingLevel")]
    thinking_level: Option<String>,

    /// Direct thinking budget; present is a 400 naming the field.
    #[serde(default, alias = "thinkingBudget")]
    thinking_budget: Option<IgnoredAny>,

    /// Thinking configuration object.
    #[serde(default, alias = "thinkingConfig")]
    thinking_config: Option<ThinkingConfigIn>,

    /// Response MIME type constraining the response format.
    #[serde(default, alias = "responseMimeType")]
    response_mime_type: Option<String>,

    /// JSON Schema the response must conform to.
    #[serde(default, alias = "responseSchema")]
    response_schema: Option<Value>,

    /// Known-but-unsupported speech configuration; present is a 400 naming the field.
    #[serde(default, alias = "speechConfig")]
    speech_config: Option<IgnoredAny>,

    /// Known-but-unsupported audio configuration; present is a 400 naming the field.
    #[serde(default, alias = "audioConfig")]
    audio_config: Option<IgnoredAny>,

    /// Known-but-unsupported video configuration; present is a 400 naming the field.
    #[serde(default, alias = "videoConfig")]
    video_config: Option<IgnoredAny>,
}

/// Thinking configuration: the level slot plus the legacy budget slot.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThinkingConfigIn {
    /// Reasoning level in the closed shared vocabulary.
    #[serde(default, alias = "thinkingLevel")]
    thinking_level: Option<String>,

    /// Legacy numeric mechanism; present is a 400 naming the field.
    #[serde(default, alias = "thinkingBudget")]
    thinking_budget: Option<IgnoredAny>,
}

/// Parses a GenerateContent request body with the router's route context.
///
/// The model comes exclusively from `route.public_id` (missing is a 400 naming `model`);
/// streaming comes exclusively from `route.is_streaming` (the URL action decides).
///
/// # Errors
///
/// Returns [`InvalidRequest`] for malformed JSON, unknown fields, unsupported semantic
/// controls, invalid image references, and invalid parameter values.
pub(super) fn parse_request(
    body: &[u8],
    route: &RouteContext,
) -> Result<ParsedRequest, InvalidRequest> {
    let Some(public_id) = route.public_id.clone() else {
        return Err(InvalidRequest::field(
            "model",
            "Missing model in request URL",
        ));
    };

    let is_streaming = route.is_streaming.unwrap_or(false);

    let request: RequestBody = crate::format::parse_client_request(body)?;

    reject_top_level_controls(&request)?;

    let instructions = parse_system_instruction(request.system_instruction.as_ref())?;
    let mut affinity =
        crate::format::affinity::Sources::new(crate::format::UpstreamFormat::GeminiGenerateContent);
    let messages = parse_contents(&request.contents, &mut affinity)?;
    let tools = parse_tools(request.tools.as_deref())?;
    let tool_choice = parse_tool_config(request.tool_config.as_ref(), tools.is_empty())?;
    let (temperature, top_p, max_output_tokens, reasoning_effort, response_format) =
        parse_generation_config(request.generation_config.as_ref())?;

    crate::format::validate_sampling_ranges(temperature, top_p)?;

    Ok(ParsedRequest {
        affinity,
        ir: IrRequest {
            has_compaction_trigger: false,
            state_items: Vec::new(),
            model: public_id,
            instructions,
            messages,
            tools,
            tool_choice,
            temperature,
            top_p,
            max_output_tokens,
            response_format,
            reasoning_effort,
        },
        is_streaming,
        should_include_usage: false,
        compaction: None,
    })
}

/// A known-but-unsupported control rejected identically at the body and generation-config
/// levels: the shared rejection vocabulary, owned here once.
#[derive(Debug, Clone, Copy)]
enum UnsupportedControl {
    /// The `stop` control.
    Stop,

    /// The `stopSequences` control.
    StopSequences,

    /// The `seed` control.
    Seed,

    /// The `speechConfig` control.
    SpeechConfig,

    /// The `audioConfig` control.
    AudioConfig,

    /// The `videoConfig` control.
    VideoConfig,
}

impl UnsupportedControl {
    /// The rejection's parameter name and message for this control.
    fn rejection(self) -> (&'static str, &'static str) {
        match self {
            Self::Stop => (
                "stop",
                "Unsupported field `stop`; stop sequences are not supported",
            ),
            Self::StopSequences => (
                "stopSequences",
                "Unsupported field `stopSequences`; stop sequences are not supported",
            ),
            Self::Seed => ("seed", "Unsupported field `seed`"),
            Self::SpeechConfig => ("speechConfig", "Unsupported field `speechConfig`"),
            Self::AudioConfig => ("audioConfig", "Unsupported field `audioConfig`"),
            Self::VideoConfig => ("videoConfig", "Unsupported field `videoConfig`"),
        }
    }
}

/// Rejects one known-but-unsupported control when present, naming its field with the shared
/// vocabulary.
fn reject_unsupported_control(
    is_present: bool,
    control: UnsupportedControl,
) -> Result<(), InvalidRequest> {
    if is_present {
        let (field, detail) = control.rejection();

        return Err(InvalidRequest::field(field, detail));
    }

    Ok(())
}

/// Rejects known-but-unsupported top-level controls, each naming its field.
fn reject_top_level_controls(request: &RequestBody) -> Result<(), InvalidRequest> {
    reject_unsupported_control(request.stop.is_some(), UnsupportedControl::Stop)?;
    reject_unsupported_control(
        request.stop_sequences.is_some(),
        UnsupportedControl::StopSequences,
    )?;
    reject_unsupported_control(request.seed.is_some(), UnsupportedControl::Seed)?;
    reject_unsupported_control(
        request.speech_config.is_some(),
        UnsupportedControl::SpeechConfig,
    )?;
    reject_unsupported_control(
        request.audio_config.is_some(),
        UnsupportedControl::AudioConfig,
    )?;
    reject_unsupported_control(
        request.video_config.is_some(),
        UnsupportedControl::VideoConfig,
    )?;

    Ok(())
}

/// Parses the system instruction into concatenated instructions.
fn parse_system_instruction(
    instruction: Option<&SystemInstructionIn>,
) -> Result<Option<String>, InvalidRequest> {
    let Some(instruction) = instruction else {
        return Ok(None);
    };

    let combined =
        crate::format::join_text_parts(instruction.parts.iter().map(|part| part.text.as_str()));

    if combined.is_empty() {
        return Ok(None);
    }

    Ok(Some(combined))
}

/// Parses conversation contents into IR messages.
fn parse_contents(
    contents: &[ContentIn],
    affinity: &mut crate::format::affinity::Sources,
) -> Result<Vec<IrMessage>, InvalidRequest> {
    if contents.is_empty() {
        return Err(InvalidRequest::field(
            "contents",
            "The request must carry at least one content",
        ));
    }

    let mut messages: Vec<IrMessage> = Vec::new();
    // Pairing channel for id-less tool results: the tool calls parsed so far, so a
    // `functionResponse` without an `id` reuses the most recent unmatched call of the same
    // name instead of minting an unresolvable id.
    let mut call_registry: Vec<CallRecord> = Vec::new();

    for content in contents {
        match content.role.as_str() {
            "user" => parse_user_content(content, &mut messages, &mut call_registry, affinity)?,
            "model" => parse_model_content(content, &mut messages, &mut call_registry, affinity)?,
            other => {
                return Err(InvalidRequest::field(
                    "contents",
                    format!("Unknown content role `{other}`"),
                ));
            }
        }
    }

    Ok(messages)
}

/// One parsed tool call awaiting its tool result: the pairing record that resolves an id-less
/// `functionResponse` to the most recent unmatched call of the same name.
struct CallRecord {
    /// Invoked function name.
    name: String,

    /// Call id carried from the wire or minted for an id-less call.
    call_id: String,

    /// Whether a tool result has already consumed this call.
    is_matched: bool,
}

/// Parses one `user` content into user and tool messages.
fn parse_user_content(
    content: &ContentIn,
    messages: &mut Vec<IrMessage>,
    call_registry: &mut [CallRecord],
    affinity: &mut crate::format::affinity::Sources,
) -> Result<(), InvalidRequest> {
    // `functionCall` parts belong to `model` contents alone; an image beside a tool result is
    // an ordinary image part, and only media inside a `functionResponse` itself is rejected
    // below (`docs/reference/inference.md`).
    if content
        .parts
        .iter()
        .any(|part| part.function_call.is_some())
    {
        return Err(InvalidRequest::field(
            "contents",
            "functionCall parts must appear in `model` contents, not `user`",
        ));
    }

    let mut text_parts: Vec<IrPart> = Vec::new();
    let mut pending_parts = Vec::new();

    for part in &content.parts {
        validate_part_shape(part)?;

        if let Some(shared) = parse_shared_payload(part)? {
            pending_parts.push(affinity.defer_part(text_parts.len()));
            text_parts.push(shared);
        } else if let Some(response) = part.function_response.as_ref() {
            // `parts` is the official media channel on tool results; the IR tool output has
            // no media channel, so any present member is a 400 naming it
            // (`docs/reference/inference.md`).
            if response.parts.is_some() {
                return Err(InvalidRequest::field(
                    "parts",
                    "Media content inside tool results is not supported",
                ));
            }

            let call_id = resolve_response_call_id(response, call_registry);
            let shape = match &response.response {
                Value::String(_) => crate::format::affinity::ResultShape::Text,
                Value::Null => crate::format::affinity::ResultShape::JsonNull,
                _ => crate::format::affinity::ResultShape::Json,
            };
            affinity.result(
                response.id.as_deref().is_some_and(|id| !id.is_empty()),
                shape,
            );

            affinity.part(messages.len(), 0);
            messages.push(IrMessage {
                role: IrRole::Tool,
                parts: vec![IrPart::ToolOutput {
                    call_id,
                    content: flatten_function_response(response)?,
                    is_error: false,
                }],
            });
        } else {
            return Err(InvalidRequest::field(
                "contents",
                "Content parts must carry text, inlineData, fileData, or functionResponse",
            ));
        }
    }

    if !text_parts.is_empty() {
        affinity.resolve_parts(&pending_parts, messages.len());
        messages.push(IrMessage {
            role: IrRole::User,
            parts: text_parts,
        });
    }

    Ok(())
}

/// Parses one `model` content into an assistant message.
fn parse_model_content(
    content: &ContentIn,
    messages: &mut Vec<IrMessage>,
    call_registry: &mut Vec<CallRecord>,
    affinity: &mut crate::format::affinity::Sources,
) -> Result<(), InvalidRequest> {
    let mut parts: Vec<IrPart> = Vec::new();

    for part in &content.parts {
        validate_part_shape(part)?;

        if let Some(shared) = parse_shared_payload(part)? {
            affinity.part(messages.len(), parts.len());
            parts.push(shared);
        } else if let Some(call) = part.function_call.as_ref() {
            if part.function_response.is_some() {
                return Err(InvalidRequest::field(
                    "contents",
                    "Content parts must carry exactly one payload",
                ));
            }

            let args_object = call.args.clone().unwrap_or(Value::Object(Map::new()));

            if !args_object.is_object() {
                return Err(InvalidRequest::field(
                    "functionCall",
                    "functionCall args must encode a JSON object",
                ));
            }

            // An empty args object serializes as `{}`, the spelling the gateway emits for a
            // call without arguments.
            let args_text = args_object.to_string();

            let call_id = match call.id.clone() {
                Some(id) if !id.is_empty() => id,
                _ => format!("call_{}", crate::ids::new_uuid()),
            };

            affinity.call(call.id.as_deref().is_some_and(|id| !id.is_empty()));
            call_registry.push(CallRecord {
                name: call.name.clone(),
                call_id: call_id.clone(),
                is_matched: false,
            });

            affinity.part(messages.len(), parts.len());
            parts.push(IrPart::ToolCall {
                call_id,
                name: call.name.clone(),
                arguments: args_text,
                thought_signature: part.thought_signature.clone(),
            });
        } else if part.function_response.is_some() {
            return Err(InvalidRequest::field(
                "contents",
                "functionResponse parts must appear in `user` contents, not `model`",
            ));
        } else {
            return Err(InvalidRequest::field(
                "contents",
                "Content parts must carry text, inlineData, fileData, or functionCall",
            ));
        }
    }

    if parts.is_empty() {
        return Ok(());
    }

    messages.push(IrMessage {
        role: IrRole::Assistant,
        parts,
    });

    Ok(())
}

/// Converts the payload kinds both roles share — text, inline bytes, or a remote image
/// reference — into its IR part; any other payload kind yields [`None`] for the role-specific
/// handling.
fn parse_shared_payload(part: &PartIn) -> Result<Option<IrPart>, InvalidRequest> {
    if let Some(text) = part.text.as_ref() {
        return Ok(Some(IrPart::Text { text: text.clone() }));
    }

    if let Some(inline) = part.inline_data.as_ref() {
        return Ok(Some(IrPart::Image {
            image: IrImage::Inline {
                mime_type: inline.mime_type.clone(),
                data_base64: inline.data.clone(),
            },
        }));
    }

    if let Some(file) = part.file_data.as_ref() {
        return Ok(Some(IrPart::Image {
            // The IR helper names its rejections after the OpenAI member `image_url`; this
            // surface's own member is `fileData`, so the rejection is re-wrapped here
            // (`docs/reference/inference.md`: every rejection names the field).
            image: IrImage::from_url(&file.file_uri)
                .map_err(|error| InvalidRequest::field("fileData", error.to_string()))?,
        }));
    }

    Ok(None)
}

/// Validates that a part carries exactly one payload member, with the gateway-emitted
/// companions in their legal positions.
fn validate_part_shape(part: &PartIn) -> Result<(), InvalidRequest> {
    let mut count = 0;

    if part.text.is_some() {
        count += 1;
    }

    if part.inline_data.is_some() {
        count += 1;
    }

    if part.file_data.is_some() {
        count += 1;
    }

    if part.function_call.is_some() {
        count += 1;
    }

    if part.function_response.is_some() {
        count += 1;
    }

    if count != 1 {
        return Err(InvalidRequest::field(
            "contents",
            "Content parts must carry exactly one payload",
        ));
    }

    if part.thought_signature.is_some() && part.function_call.is_none() {
        return Err(InvalidRequest::field(
            "thoughtSignature",
            "thoughtSignature is valid only beside functionCall",
        ));
    }

    Ok(())
}

/// Resolves the call id a tool result answers: the explicit `id` when present, else the most
/// recent unmatched call of the same name, else a freshly minted id.
fn resolve_response_call_id(
    response: &FunctionResponseIn,
    call_registry: &mut [CallRecord],
) -> String {
    if let Some(id) = response.id.clone()
        && !id.is_empty()
    {
        return id;
    }

    if let Some(record) = call_registry
        .iter_mut()
        .rev()
        .find(|record| record.name == response.name && !record.is_matched)
    {
        record.is_matched = true;

        return record.call_id.clone();
    }

    format!("call_{}", crate::ids::new_uuid())
}

/// Flattens a function response payload to JSON text: objects and other structured values
/// serialize, strings ride verbatim.
fn flatten_function_response(response: &FunctionResponseIn) -> Result<String, InvalidRequest> {
    match &response.response {
        Value::String(text) => Ok(text.clone()),
        Value::Null => Ok(String::new()),
        other => Ok(other.to_string()),
    }
}

/// Parses advertised tools: function declarations only.
fn parse_tools(tools: Option<&[ToolIn]>) -> Result<Vec<IrTool>, InvalidRequest> {
    let Some(tools) = tools else {
        return Ok(Vec::new());
    };

    let mut parsed: Vec<IrTool> = Vec::with_capacity(tools.len());

    for group in tools {
        let Some(declarations) = group.function_declarations.as_ref() else {
            return Err(InvalidRequest::field(
                "tools",
                "Only function tools are supported; built-in tools are not supported",
            ));
        };

        if declarations.is_empty() {
            return Err(InvalidRequest::field(
                "tools",
                "functionDeclarations must carry at least one declaration",
            ));
        }

        for declaration in declarations {
            if declaration.name.is_empty() {
                return Err(InvalidRequest::field(
                    "tools",
                    "Function declarations must carry a name",
                ));
            }

            parsed.push(IrTool {
                name: declaration.name.clone(),
                description: declaration.description.clone(),
                parameters: declaration.parameters.clone(),
            });
        }
    }

    Ok(parsed)
}

/// Parses the tool selection configuration.
fn parse_tool_config(
    config: Option<&ToolConfigIn>,
    is_tools_empty: bool,
) -> Result<Option<IrToolChoice>, InvalidRequest> {
    let Some(config) = config else {
        return Ok(None);
    };

    if is_tools_empty {
        return Err(InvalidRequest::field(
            "toolConfig",
            "toolConfig requires at least one tool",
        ));
    }

    let allowed = config
        .function_calling_config
        .allowed_function_names
        .clone()
        .unwrap_or_default();

    match config.function_calling_config.mode.as_str() {
        "AUTO" => {
            if !allowed.is_empty() {
                return Err(InvalidRequest::field(
                    "toolConfig",
                    "AUTO mode must not carry allowedFunctionNames",
                ));
            }

            Ok(Some(IrToolChoice::Auto))
        }

        "NONE" => {
            if !allowed.is_empty() {
                return Err(InvalidRequest::field(
                    "toolConfig",
                    "NONE mode must not carry allowedFunctionNames",
                ));
            }

            Ok(Some(IrToolChoice::None))
        }

        "ANY" => match allowed.len() {
            0 => Ok(Some(IrToolChoice::Required)),
            1 => {
                let name = allowed.first().cloned().unwrap_or_default();

                Ok(Some(IrToolChoice::Function { name }))
            }
            _ => Err(InvalidRequest::field(
                "toolConfig",
                "ANY mode supports at most one allowed function",
            )),
        },

        other => Err(InvalidRequest::field(
            "toolConfig",
            format!("Unknown toolConfig mode `{other}`"),
        )),
    }
}

/// Parses sampling, reasoning, and response-format controls from the generation config.
#[expect(
    clippy::type_complexity,
    reason = "the generation config yields five IR controls at once"
)]
fn parse_generation_config(
    config: Option<&GenerationConfigIn>,
) -> Result<
    (
        Option<f64>,
        Option<f64>,
        Option<u64>,
        Option<ReasoningLevel>,
        Option<IrResponseFormat>,
    ),
    InvalidRequest,
> {
    let Some(config) = config else {
        return Ok((None, None, None, None, None));
    };

    reject_unsupported_control(config.stop.is_some(), UnsupportedControl::Stop)?;
    reject_unsupported_control(
        config.stop_sequences.is_some(),
        UnsupportedControl::StopSequences,
    )?;
    reject_unsupported_control(config.seed.is_some(), UnsupportedControl::Seed)?;

    if config.top_k.is_some() {
        return Err(InvalidRequest::field("topK", "Unsupported field `topK`"));
    }

    reject_unsupported_control(
        config.speech_config.is_some(),
        UnsupportedControl::SpeechConfig,
    )?;
    reject_unsupported_control(
        config.audio_config.is_some(),
        UnsupportedControl::AudioConfig,
    )?;
    reject_unsupported_control(
        config.video_config.is_some(),
        UnsupportedControl::VideoConfig,
    )?;

    if config.thinking_budget.is_some() {
        return Err(InvalidRequest::field(
            "thinkingBudget",
            "Unsupported field `thinkingBudget`; use thinkingLevel instead",
        ));
    }

    if let Some(thinking) = config.thinking_config.as_ref()
        && thinking.thinking_budget.is_some()
    {
        return Err(InvalidRequest::field(
            "thinkingBudget",
            "Unsupported field `thinkingBudget`; use thinkingLevel instead",
        ));
    }

    let direct_level = config.thinking_level.as_deref();
    let nested_level = config
        .thinking_config
        .as_ref()
        .and_then(|thinking| thinking.thinking_level.as_deref());

    let reasoning_effort = match (direct_level, nested_level) {
        (Some(_), Some(_)) => {
            return Err(InvalidRequest::field(
                "thinkingLevel",
                "thinkingLevel must appear either directly or in thinkingConfig, not both",
            ));
        }

        (Some(level), None) | (None, Some(level)) => {
            Some(ReasoningLevel::parse(level, "thinkingLevel")?)
        }

        (None, None) => None,
    };

    let response_format = parse_response_format(
        config.response_mime_type.as_deref(),
        config.response_schema.as_ref(),
    )?;

    Ok((
        config.temperature,
        config.top_p,
        config.max_output_tokens,
        reasoning_effort,
        response_format,
    ))
}

/// Parses the response MIME type plus schema into the IR response format: the shared family table
/// over a present MIME type (see [`parse_gemini_response_format`]) plus this surface's
/// absent-MIME-type cells — an absent `responseMimeType` beside no schema is no response format at
/// all, and a schema beside the absent member is a rejection naming the member. Every rejection
/// names the wire member this surface speaks.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `responseMimeType` or `responseSchema`.
fn parse_response_format(
    mime_type: Option<&str>,
    schema: Option<&Value>,
) -> Result<Option<IrResponseFormat>, InvalidRequest> {
    let (mime_type, schema) = match (mime_type, schema) {
        (None, None) => return Ok(None),

        (None, Some(_)) => {
            return Err(InvalidRequest::field(
                "responseMimeType",
                "responseSchema requires responseMimeType `application/json`",
            ));
        }

        (Some(mime_type), schema) => (mime_type, schema),
    };

    parse_gemini_response_format(mime_type, schema).map_err(|rejection| match rejection {
        GeminiResponseFormatRejection::SchemaWithoutJsonMime => InvalidRequest::field(
            "responseSchema",
            "responseSchema requires responseMimeType `application/json`",
        ),

        GeminiResponseFormatRejection::UnsupportedMime(mime_type) => InvalidRequest::field(
            "responseMimeType",
            format!("Unsupported responseMimeType `{mime_type}`"),
        ),
    })
}
