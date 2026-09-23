//! Gemini GenerateContent inbound surface (`/api/gemini-v1beta/models/...`).
//!
//! Implements the [`ClientFormat`] for Gemini GenerateContent —
//! `POST /models/{model}:generateContent` (non-streaming) and
//! `POST /models/{model}:streamGenerateContent?alt=sse` (streaming, only the `alt=sse` form is
//! supported, else 400) plus `GET /models` — per REFACTOR §§4–5 and README §§6.4–6.6.
//!
//! The architecture this surface plugs into:
//! - [`crate::format::RouteContext`] carries the URL-routed public model id and the streaming mode
//!   from the URL action; the body carries no model.
//! - [`crate::ir::IrReasoningLevel`] for the inbound level parameter (unknown strings are a 400);
//! - [`crate::ir::IrPart::Reasoning`] never originates here (GenerateContent has no reasoning input
//!   slot); history-replay carve-outs stay minimal — only what the gateway itself emits
//!   (`thoughtSignature` on `functionCall` parts, gateway-rendered `id` fields, and the `thought`
//!   flag accepted-and-ignored as plain text);
//! - [`crate::api::Error`] with [`crate::format::error_gemini`] for both error paths;
//! - [`crate::api_logs::ApiLogEndpointType::GeminiGenerateContent`] as `ENDPOINT_TYPE`.
//!
//! Wire shapes follow the official Gemini GenerateContent REST shapes in their camelCase
//! spellings (`systemInstruction`, `generationConfig`, `toolConfig`, `functionCall`,
//! `functionResponse`, `inlineData`, `fileData`, `thoughtSignature`). Every field additionally
//! accepts its snake_case alias (`system_instruction`, `generation_config`, …) so callers using
//! either convention parse identically; truly unknown fields remain 400s through
//! `deny_unknown_fields`.

use crate::{
    api_logs::ApiLogEndpointType,
    format::{ClientFormat, ClientSseRenderer, ParsedRequest, RouteContext},
    ir::{
        InvalidRequest, IrAggregation, IrEvent, IrFinishReason, IrImage, IrMessage, IrPart,
        IrReasoningLevel, IrRequest, IrResponseFormat, IrRole, IrTool, IrToolChoice, IrUsage,
    },
};
use serde::{Deserialize, Serialize, de::IgnoredAny};
use serde_json::{Map, Value};

/// Marker of the inbound Gemini GenerateContent client format.
#[derive(Debug)]
pub(crate) struct Format;

impl ClientFormat for Format {
    type JsonResponse = JsonResponse;

    const ENDPOINT_TYPE: ApiLogEndpointType = ApiLogEndpointType::GeminiGenerateContent;

    fn parse_request(body: &[u8], route: &RouteContext) -> Result<ParsedRequest, InvalidRequest> {
        parse_request(body, route)
    }

    fn sse_renderer(
        _should_include_usage: bool,
        public_id: String,
    ) -> Box<dyn ClientSseRenderer + Send> {
        Box::new(SseRenderer::new(public_id))
    }

    fn render_json_response(public_id: &str, aggregation: &IrAggregation) -> Self::JsonResponse {
        render_json_response(public_id, aggregation)
    }
}

// ---------------------------------------------------------------------------
// Outbound rendering: native GenerateContent shapes.
// ---------------------------------------------------------------------------

/// Aggregated non-streaming GenerateContent success body: one candidate plus usage.
#[derive(Debug, Serialize)]
pub(crate) struct JsonResponse {
    /// Candidate answers; the gateway serves exactly one.
    pub(crate) candidates: Vec<CandidateOut>,

    /// Token accounting in the Gemini shape.
    #[serde(rename = "usageMetadata")]
    pub(crate) usage_metadata: UsageMetadataOut,

    /// Public model id the client addressed, echoed as the upstream model version.
    #[serde(rename = "modelVersion")]
    pub(crate) model_version: String,

    /// Gateway-minted response identifier.
    #[serde(rename = "responseId")]
    pub(crate) response_id: String,
}

/// One rendered candidate answer.
#[derive(Debug, Serialize)]
pub(crate) struct CandidateOut {
    /// Model content of the candidate.
    pub(crate) content: ContentOut,

    /// Canonical finish reason in the Gemini vocabulary.
    #[serde(rename = "finishReason")]
    pub(crate) finish_reason: &'static str,

    /// Candidate index; the gateway serves exactly one.
    pub(crate) index: u32,
}

/// Model content of one candidate.
#[derive(Debug, Serialize)]
pub(crate) struct ContentOut {
    /// Always `model` for outbound candidates.
    pub(crate) role: &'static str,

    /// Ordered parts of the candidate.
    pub(crate) parts: Vec<PartOut>,
}

/// One outbound part: text (or thought) or a function call.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum PartOut {
    /// Plain text or reasoning text (`thought` set for reasoning).
    Text {
        /// Text content.
        text: String,

        /// Set exactly for reasoning text; omitted for answer text.
        #[serde(skip_serializing_if = "Option::is_none")]
        thought: Option<bool>,
    },

    /// Assistant tool invocation with its replay signature.
    FunctionCall {
        /// Invocation payload.
        #[serde(rename = "functionCall")]
        function_call: FunctionCallOut,

        /// Upstream thought signature replayed across turns.
        #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
}

/// Outbound function invocation payload.
#[derive(Debug, Serialize)]
pub(crate) struct FunctionCallOut {
    /// Invoked function name.
    pub(crate) name: String,

    /// Arguments object.
    pub(crate) args: Map<String, Value>,

    /// Invocation identifier, the cross-turn pairing channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<String>,
}

/// Token accounting in the Gemini shape.
#[derive(Debug, Serialize)]
pub(crate) struct UsageMetadataOut {
    /// Tokens consumed by the prompt.
    #[serde(rename = "promptTokenCount")]
    pub(crate) prompt_token_count: u64,

    /// Tokens produced by candidates, excluding reasoning.
    #[serde(rename = "candidatesTokenCount")]
    pub(crate) candidates_token_count: u64,

    /// Tokens spent on reasoning, omitted when unreported.
    #[serde(rename = "thoughtsTokenCount", skip_serializing_if = "Option::is_none")]
    pub(crate) thoughts_token_count: Option<u64>,

    /// Prompt tokens served from cache, omitted when unreported.
    #[serde(
        rename = "cachedContentTokenCount",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) cached_content_token_count: Option<u64>,
}

/// Streaming chunk: a partial GenerateContentResponse.
#[derive(Debug, Serialize)]
struct StreamChunk {
    /// Partial candidates of this chunk.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    candidates: Vec<StreamCandidate>,

    /// Cumulative usage, carried only by the terminal usage chunk.
    #[serde(rename = "usageMetadata", skip_serializing_if = "Option::is_none")]
    usage_metadata: Option<UsageMetadataOut>,

    /// Public model id echoed as the upstream model version.
    #[serde(rename = "modelVersion")]
    model_version: String,

    /// Gateway-minted response identifier, stable across the stream.
    #[serde(rename = "responseId")]
    response_id: String,
}

/// One streaming candidate: partial content with an optional terminal reason.
#[derive(Debug, Serialize)]
struct StreamCandidate {
    /// Partial model content.
    content: ContentOut,

    /// Terminal reason, present only on the finish chunk.
    #[serde(rename = "finishReason", skip_serializing_if = "Option::is_none")]
    finish_reason: Option<&'static str>,

    /// Always `0`: the gateway serves one candidate.
    index: u32,
}

/// Maps the canonical finish reason to the Gemini vocabulary.
///
/// Gemini ends function-call turns with the plain `STOP` reason — its vocabulary has no
/// tool-call value — so a turn that delivered calls still reports `STOP`, matching the
/// upstream parser's inverse mapping.
fn finish_reason_to_gemini(reason: IrFinishReason) -> &'static str {
    match reason {
        IrFinishReason::Stop | IrFinishReason::ToolCall => "STOP",
        IrFinishReason::Length => "MAX_TOKENS",
        IrFinishReason::ContentFilter => "SAFETY",
    }
}

/// Renders IR usage into the Gemini usage shape: the output total keeps including reasoning
/// (matching the upstream codec), so the candidates count is the output minus reasoning.
fn usage_to_gemini(usage: IrUsage) -> UsageMetadataOut {
    let reasoning = usage.reasoning_tokens.unwrap_or(0);

    UsageMetadataOut {
        prompt_token_count: usage.input_tokens,
        candidates_token_count: usage.output_tokens.saturating_sub(reasoning),
        thoughts_token_count: usage.reasoning_tokens,
        cached_content_token_count: usage.cached_tokens,
    }
}

/// Renders tool-call arguments text into the object the wire carries; an empty string is an
/// empty object and unparseable input (which conformant upstreams never produce) degrades to
/// an empty object rather than failing a healthy stream.
fn arguments_to_object(arguments: &str) -> Map<String, Value> {
    if arguments.trim().is_empty() {
        return Map::new();
    }

    match serde_json::from_str::<Value>(arguments) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// Renders an aggregated completion as a non-streaming GenerateContent response.
fn render_json_response(public_id: &str, aggregation: &IrAggregation) -> JsonResponse {
    let mut parts: Vec<PartOut> = Vec::new();

    if !aggregation.reasoning.is_empty() {
        parts.push(PartOut::Text {
            text: aggregation.reasoning.clone(),
            thought: Some(true),
        });
    }

    if !aggregation.text.is_empty() {
        parts.push(PartOut::Text {
            text: aggregation.text.clone(),
            thought: None,
        });
    }

    for call in &aggregation.tool_calls {
        parts.push(PartOut::FunctionCall {
            function_call: FunctionCallOut {
                name: call.name.clone(),
                args: arguments_to_object(&call.arguments),
                id: Some(call.call_id.clone()),
            },
            thought_signature: call.thought_signature.clone(),
        });
    }

    let usage = aggregation.usage.unwrap_or_default();

    JsonResponse {
        candidates: vec![CandidateOut {
            content: ContentOut {
                role: "model",
                parts,
            },
            finish_reason: finish_reason_to_gemini(
                aggregation.finish.unwrap_or(IrFinishReason::Stop),
            ),
            index: 0,
        }],
        usage_metadata: usage_to_gemini(usage),
        model_version: public_id.to_string(),
        response_id: format!("gen-{}", crate::ids::new_uuid()),
    }
}

/// Streaming renderer producing native GenerateContent SSE frames.
#[derive(Debug)]
struct SseRenderer {
    /// Public model id echoed as `modelVersion`.
    model: String,

    /// Gateway-minted response identifier, stable across the stream.
    response_id: String,

    /// Whether any reasoning delta was already rendered; a non-empty done renders only when
    /// none preceded it (parsers emit either deltas plus an empty done, or a done carrying the
    /// full text with no deltas — never both with overlapping text).
    is_reasoning_streamed: bool,

    /// Latest usage observed; rendered once terminally.
    usage: Option<IrUsage>,
}

impl SseRenderer {
    /// Creates a renderer for one streamed completion.
    fn new(model: String) -> Self {
        Self {
            model,
            response_id: format!("gen-{}", crate::ids::new_uuid()),
            is_reasoning_streamed: false,
            usage: None,
        }
    }

    /// Formats one streaming chunk as a bare `data:` frame.
    fn chunk_frame(
        &self,
        candidates: Vec<StreamCandidate>,
        usage: Option<UsageMetadataOut>,
    ) -> String {
        let chunk = StreamChunk {
            candidates,
            usage_metadata: usage,
            model_version: self.model.clone(),
            response_id: self.response_id.clone(),
        };

        crate::sse::format_data_frame(&crate::format::render_json(&chunk))
    }

    /// Formats one content chunk carrying the given parts.
    fn parts_frame(&self, parts: Vec<PartOut>) -> String {
        self.chunk_frame(
            vec![StreamCandidate {
                content: ContentOut {
                    role: "model",
                    parts,
                },
                finish_reason: None,
                index: 0,
            }],
            None,
        )
    }
}

impl ClientSseRenderer for SseRenderer {
    /// Renders one IR event into zero or more SSE frames.
    fn render(&mut self, event: &crate::ir::IrEvent) -> Vec<String> {
        match event {
            IrEvent::MessageStart { .. } => Vec::new(),

            IrEvent::TextDelta(delta) => vec![self.parts_frame(vec![PartOut::Text {
                text: delta.clone(),
                thought: None,
            }])],

            IrEvent::ReasoningDelta(delta) => {
                self.is_reasoning_streamed = true;

                vec![self.parts_frame(vec![PartOut::Text {
                    text: delta.clone(),
                    thought: Some(true),
                }])]
            }

            // A done carrying text renders it only when no deltas preceded it; the opaque
            // payload is dropped on this surface.
            IrEvent::ReasoningDone { text, .. } => {
                if text.is_empty() || self.is_reasoning_streamed {
                    Vec::new()
                } else {
                    self.is_reasoning_streamed = true;

                    vec![self.parts_frame(vec![PartOut::Text {
                        text: text.clone(),
                        thought: Some(true),
                    }])]
                }
            }

            // Tool-call deltas buffer upstream: the done carries the complete call, so deltas
            // render nothing and the done renders the full invocation live.
            IrEvent::ToolCallDelta { .. } => Vec::new(),

            IrEvent::ToolCallDone {
                call_id,
                name,
                arguments,
                thought_signature,
                ..
            } => vec![self.parts_frame(vec![PartOut::FunctionCall {
                function_call: FunctionCallOut {
                    name: name.clone(),
                    args: arguments_to_object(arguments),
                    id: Some(call_id.clone()),
                },
                thought_signature: thought_signature.clone(),
            }])],

            IrEvent::Finish(reason) => vec![self.chunk_frame(
                vec![StreamCandidate {
                    content: ContentOut {
                        role: "model",
                        parts: Vec::new(),
                    },
                    finish_reason: Some(finish_reason_to_gemini(*reason)),
                    index: 0,
                }],
                None,
            )],

            IrEvent::Usage(usage) => {
                self.usage = Some(*usage);

                Vec::new()
            }

            IrEvent::Done => match self.usage.take() {
                Some(usage) => vec![self.chunk_frame(Vec::new(), Some(usage_to_gemini(usage)))],
                None => Vec::new(),
            },
        }
    }

    /// Renders a mid-stream error as a terminal SSE frame carrying the Gemini error envelope.
    fn error_frame(&mut self, error: &crate::api::Error) -> String {
        let (_, envelope) = crate::format::error_gemini::render(error);

        crate::sse::format_data_frame(&crate::format::render_json(&envelope))
    }
}

// ---------------------------------------------------------------------------
// Inbound parsing: strict DTOs.
// ---------------------------------------------------------------------------

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
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
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
fn parse_request(body: &[u8], route: &RouteContext) -> Result<ParsedRequest, InvalidRequest> {
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
    let messages = parse_contents(&request.contents)?;
    let tools = parse_tools(request.tools.as_deref())?;
    let tool_choice = parse_tool_config(request.tool_config.as_ref(), tools.is_empty())?;
    let (temperature, top_p, max_output_tokens, reasoning_effort, response_format) =
        parse_generation_config(request.generation_config.as_ref())?;

    crate::format::validate_sampling_ranges(temperature, top_p)?;

    Ok(ParsedRequest {
        ir: IrRequest {
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
    })
}

/// Rejects known-but-unsupported top-level controls, each naming its field.
fn reject_top_level_controls(request: &RequestBody) -> Result<(), InvalidRequest> {
    if request.stop.is_some() {
        return Err(InvalidRequest::field(
            "stop",
            "Unsupported field `stop`; stop sequences are not supported",
        ));
    }

    if request.stop_sequences.is_some() {
        return Err(InvalidRequest::field(
            "stopSequences",
            "Unsupported field `stopSequences`; stop sequences are not supported",
        ));
    }

    if request.seed.is_some() {
        return Err(InvalidRequest::field("seed", "Unsupported field `seed`"));
    }

    if request.speech_config.is_some() {
        return Err(InvalidRequest::field(
            "speechConfig",
            "Unsupported field `speechConfig`",
        ));
    }

    if request.audio_config.is_some() {
        return Err(InvalidRequest::field(
            "audioConfig",
            "Unsupported field `audioConfig`",
        ));
    }

    if request.video_config.is_some() {
        return Err(InvalidRequest::field(
            "videoConfig",
            "Unsupported field `videoConfig`",
        ));
    }

    Ok(())
}

/// Parses the system instruction into concatenated instructions.
fn parse_system_instruction(
    instruction: Option<&SystemInstructionIn>,
) -> Result<Option<String>, InvalidRequest> {
    let Some(instruction) = instruction else {
        return Ok(None);
    };

    let mut combined = String::new();

    for part in &instruction.parts {
        if !combined.is_empty() {
            combined.push_str("\n\n");
        }

        combined.push_str(&part.text);
    }

    if combined.is_empty() {
        return Ok(None);
    }

    Ok(Some(combined))
}

/// Parses conversation contents into IR messages.
fn parse_contents(contents: &[ContentIn]) -> Result<Vec<IrMessage>, InvalidRequest> {
    if contents.is_empty() {
        return Err(InvalidRequest::field(
            "contents",
            "The request must carry at least one content",
        ));
    }

    let mut messages: Vec<IrMessage> = Vec::new();
    // Pairing channel for id-less tool results: the call ids of tool calls seen so far,
    // with their function names, so a `functionResponse` without an `id` reuses the most
    // recent unmatched call of the same name instead of minting an unresolvable id.
    let mut call_registry: Vec<(String, String)> = Vec::new();
    let mut matched_calls: Vec<bool> = Vec::new();

    for content in contents {
        match content.role.as_str() {
            "user" => parse_user_content(
                content,
                &mut messages,
                &mut call_registry,
                &mut matched_calls,
            )?,
            "model" => parse_model_content(
                content,
                &mut messages,
                &mut call_registry,
                &mut matched_calls,
            )?,
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

/// Parses one `user` content into user and tool messages.
fn parse_user_content(
    content: &ContentIn,
    messages: &mut Vec<IrMessage>,
    call_registry: &mut [(String, String)],
    matched_calls: &mut [bool],
) -> Result<(), InvalidRequest> {
    let mut has_function_call = false;
    let mut has_function_response = false;
    let mut has_image = false;

    for part in &content.parts {
        if part.function_call.is_some() {
            has_function_call = true;
        }

        if part.function_response.is_some() {
            has_function_response = true;
        }

        if part.inline_data.is_some() || part.file_data.is_some() {
            has_image = true;
        }
    }

    if has_function_call {
        return Err(InvalidRequest::field(
            "contents",
            "functionCall parts must appear in `model` contents, not `user`",
        ));
    }

    if has_function_response && has_image {
        return Err(InvalidRequest::field(
            "functionResponse",
            "Image content inside tool results is not supported",
        ));
    }

    let mut text_parts: Vec<IrPart> = Vec::new();

    for part in &content.parts {
        validate_part_shape(part)?;

        if let Some(text) = part.text.as_ref() {
            text_parts.push(IrPart::Text { text: text.clone() });
        } else if let Some(inline) = part.inline_data.as_ref() {
            text_parts.push(IrPart::Image {
                image: IrImage::Inline {
                    mime_type: inline.mime_type.clone(),
                    data_base64: inline.data.clone(),
                },
            });
        } else if let Some(file) = part.file_data.as_ref() {
            text_parts.push(IrPart::Image {
                image: IrImage::from_url(&file.file_uri)?,
            });
        } else if let Some(response) = part.function_response.as_ref() {
            let call_id =
                resolve_response_call_id(response, &response.name, call_registry, matched_calls);

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
        messages.push(IrMessage {
            role: IrRole::User,
            parts: text_parts,
        });
    }

    Ok(())
}

/// Parses one `model` content into an assistant message.
#[allow(
    clippy::ptr_arg,
    reason = "the registry grows with each tool call, so it must stay a Vec"
)]
fn parse_model_content(
    content: &ContentIn,
    messages: &mut Vec<IrMessage>,
    call_registry: &mut Vec<(String, String)>,
    matched_calls: &mut Vec<bool>,
) -> Result<(), InvalidRequest> {
    let mut parts: Vec<IrPart> = Vec::new();

    for part in &content.parts {
        validate_part_shape(part)?;

        if let Some(text) = part.text.as_ref() {
            parts.push(IrPart::Text { text: text.clone() });
        } else if let Some(inline) = part.inline_data.as_ref() {
            parts.push(IrPart::Image {
                image: IrImage::Inline {
                    mime_type: inline.mime_type.clone(),
                    data_base64: inline.data.clone(),
                },
            });
        } else if let Some(file) = part.file_data.as_ref() {
            parts.push(IrPart::Image {
                image: IrImage::from_url(&file.file_uri)?,
            });
        } else if let Some(call) = part.function_call.as_ref() {
            if part.function_response.is_some() {
                return Err(InvalidRequest::field(
                    "contents",
                    "Content parts must carry exactly one payload",
                ));
            }

            let args_value = call.args.clone().unwrap_or(Value::Object(Map::new()));

            let args_object = match &args_value {
                Value::Object(_) => args_value,
                _ => {
                    return Err(InvalidRequest::field(
                        "functionCall",
                        "functionCall args must encode a JSON object",
                    ));
                }
            };

            let args_text = if args_object.as_object().is_some_and(Map::is_empty) {
                "{}".to_string()
            } else {
                args_object.to_string()
            };

            let call_id = match call.id.clone() {
                Some(id) if !id.is_empty() => id,
                _ => format!("call_{}", crate::ids::new_uuid()),
            };

            call_registry.push((call.name.clone(), call_id.clone()));
            matched_calls.push(false);

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
    name: &str,
    call_registry: &mut [(String, String)],
    matched_calls: &mut [bool],
) -> String {
    if let Some(id) = response.id.clone()
        && !id.is_empty()
    {
        return id;
    }

    for index in (0..call_registry.len()).rev() {
        let matches_name = call_registry
            .get(index)
            .is_some_and(|(call_name, _)| call_name == name);
        let is_unmatched = matched_calls.get(index).is_some_and(|matched| !matched);

        if matches_name && is_unmatched {
            if let Some(matched) = matched_calls.get_mut(index) {
                *matched = true;
            }

            if let Some((_, call_id)) = call_registry.get(index) {
                return call_id.clone();
            }
        }
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
#[allow(
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
        Option<IrReasoningLevel>,
        Option<IrResponseFormat>,
    ),
    InvalidRequest,
> {
    let Some(config) = config else {
        return Ok((None, None, None, None, None));
    };

    if config.stop.is_some() {
        return Err(InvalidRequest::field(
            "stop",
            "Unsupported field `stop`; stop sequences are not supported",
        ));
    }

    if config.stop_sequences.is_some() {
        return Err(InvalidRequest::field(
            "stopSequences",
            "Unsupported field `stopSequences`; stop sequences are not supported",
        ));
    }

    if config.seed.is_some() {
        return Err(InvalidRequest::field("seed", "Unsupported field `seed`"));
    }

    if config.top_k.is_some() {
        return Err(InvalidRequest::field("topK", "Unsupported field `topK`"));
    }

    if config.speech_config.is_some() {
        return Err(InvalidRequest::field(
            "speechConfig",
            "Unsupported field `speechConfig`",
        ));
    }

    if config.audio_config.is_some() {
        return Err(InvalidRequest::field(
            "audioConfig",
            "Unsupported field `audioConfig`",
        ));
    }

    if config.video_config.is_some() {
        return Err(InvalidRequest::field(
            "videoConfig",
            "Unsupported field `videoConfig`",
        ));
    }

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
            Some(IrReasoningLevel::parse(level, "thinkingLevel")?)
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

/// Parses the response MIME type plus schema into the IR response format.
fn parse_response_format(
    mime_type: Option<&str>,
    schema: Option<&Value>,
) -> Result<Option<IrResponseFormat>, InvalidRequest> {
    match (mime_type, schema) {
        (None, None) => Ok(None),

        (None, Some(_)) => Err(InvalidRequest::field(
            "responseMimeType",
            "responseSchema requires responseMimeType `application/json`",
        )),

        (Some(mime), response_schema) => match mime {
            "text/plain" => {
                if response_schema.is_some() {
                    return Err(InvalidRequest::field(
                        "responseSchema",
                        "responseSchema requires responseMimeType `application/json`",
                    ));
                }

                Ok(Some(IrResponseFormat::Text))
            }

            "application/json" => match response_schema {
                Some(schema) => Ok(Some(IrResponseFormat::JsonSchema {
                    name: "response".to_string(),
                    schema: schema.clone(),
                })),

                None => Ok(Some(IrResponseFormat::Json)),
            },

            other => Err(InvalidRequest::field(
                "responseMimeType",
                format!("Unsupported responseMimeType `{other}`"),
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::RouteContext;

    /// Parses one body with the given route context.
    fn parse_with_route(
        body: Value,
        route: &RouteContext,
    ) -> Result<ParsedRequest, InvalidRequest> {
        parse_request(body.to_string().as_bytes(), route)
    }

    /// Parses one non-streaming body addressed at `test-model`.
    fn parse_body(body: Value) -> Result<ParsedRequest, InvalidRequest> {
        parse_with_route(
            body,
            &RouteContext::with_model("test-model".to_string(), Some(false)),
        )
    }

    /// Minimal valid contents carrying one user text turn.
    fn user_contents() -> Value {
        serde_json::json!([
            {"role": "user", "parts": [{"text": "hi"}]},
        ])
    }

    /// Verifies the model rides the URL path exclusively: a missing route id is a 400 naming
    /// `model`, the parsed IR carries the route id, and a body carrying its own `model`
    /// member is rejected as an unknown field.
    ///
    /// # Panics
    ///
    /// Panics if the parsed model or rejection diverges from the contract.
    #[test]
    fn model_rides_the_url_path_exclusively() {
        #[expect(
            clippy::expect_used,
            reason = "the minimal body is constructed to parse"
        )]
        let parsed = parse_body(serde_json::json!({"contents": user_contents()}))
            .expect("a minimal body must parse");

        assert_eq!(parsed.ir.model, "test-model");
        assert!(!parsed.is_streaming);

        #[expect(
            clippy::expect_used,
            reason = "the missing model is constructed to fail"
        )]
        let error = parse_with_route(
            serde_json::json!({"contents": user_contents()}),
            &RouteContext::empty(),
        )
        .expect_err("a missing route model must fail");

        assert_eq!(error.param.as_deref(), Some("model"));

        #[expect(clippy::expect_used, reason = "the body model is constructed to fail")]
        let error = parse_body(serde_json::json!({"contents": user_contents(), "model": "other"}))
            .expect_err("a body carrying a model must fail");

        assert!(
            error.to_string().contains("unknown field `model`"),
            "unexpected rejection: {error}"
        );
    }

    /// Verifies streaming rides the URL action exclusively: the route flag decides the parsed
    /// mode and a body `stream` member is rejected as unknown.
    ///
    /// # Panics
    ///
    /// Panics if the parsed mode or rejection diverges from the contract.
    #[test]
    fn streaming_rides_the_url_action_exclusively() {
        let route = RouteContext::with_model("m".to_string(), Some(true));

        #[expect(
            clippy::expect_used,
            reason = "the streaming route is constructed to parse"
        )]
        let parsed = parse_with_route(serde_json::json!({"contents": user_contents()}), &route)
            .expect("a streaming route must parse as streaming");

        assert!(parsed.is_streaming);

        #[expect(
            clippy::expect_used,
            reason = "the body stream flag is constructed to fail"
        )]
        let error = parse_body(serde_json::json!({"contents": user_contents(), "stream": true}))
            .expect_err("a body carrying stream must fail");

        assert!(
            error.to_string().contains("unknown field `stream`"),
            "unexpected rejection: {error}"
        );
    }

    /// Verifies the full user/model round-trip the gateway itself emits parses back:
    /// system instructions concatenate, text parts map to IR text, `functionCall` parts
    /// (with `thoughtSignature` and `id`) map to tool calls, and object-shaped
    /// `functionResponse` results flatten to JSON text while `id`-less pairs resolve by
    /// name.
    ///
    /// # Panics
    ///
    /// Panics if the parsed IR diverges from the documented mapping.
    #[test]
    fn parses_the_gateway_emitted_history_shapes() {
        #[expect(
            clippy::expect_used,
            reason = "the history body is constructed to parse"
        )]
        let parsed = parse_body(serde_json::json!({
            "systemInstruction": {"parts": [{"text": "be terse"}, {"text": "no markdown"}]},
            "contents": [
                {"role": "user", "parts": [{"text": "run checks"}]},
                {"role": "model", "parts": [
                    {"functionCall": {"name": "lint", "args": {}}, "thoughtSignature": "sig-a"},
                    {"functionCall": {"name": "test", "args": {"fast": true}, "id": "call_b"}},
                ]},
                {"role": "user", "parts": [
                    {"functionResponse": {"name": "lint", "response": {"passed": true}}},
                    {"functionResponse": {
                        "name": "test",
                        "response": {"ok": true},
                        "id": "call_b",
                    }},
                ]},
                {"role": "user", "parts": [{"text": "thanks"}]},
            ],
        }))
        .expect("gateway-emitted history must parse");

        assert_eq!(
            parsed.ir.instructions.as_deref(),
            Some("be terse\n\nno markdown")
        );

        let tool_messages: Vec<&IrMessage> = parsed
            .ir
            .messages
            .iter()
            .filter(|message| message.role == IrRole::Tool)
            .collect();

        assert_eq!(tool_messages.len(), 2);

        #[expect(
            clippy::indexing_slicing,
            reason = "the two tool messages are asserted above"
        )]
        let first = &tool_messages[0];

        assert_eq!(first.parts.len(), 1);

        #[expect(
            clippy::indexing_slicing,
            reason = "the single tool-output part is asserted above"
        )]
        match &first.parts[0] {
            IrPart::ToolOutput { content, .. } => {
                #[expect(
                    clippy::expect_used,
                    reason = "the flattened content is constructed to be valid JSON"
                )]
                let flattened = serde_json::from_str::<Value>(content)
                    .expect("object results flatten to JSON text");

                assert_eq!(flattened, serde_json::json!({"passed": true}));
            }
            other => {
                #[expect(
                    clippy::panic,
                    reason = "a part of another kind fails the mapping under test"
                )]
                {
                    panic!("expected a tool output, got {other:?}")
                }
            }
        }

        #[expect(
            clippy::expect_used,
            reason = "the assistant turn is constructed to be present"
        )]
        let assistant = parsed
            .ir
            .messages
            .iter()
            .find(|message| message.role == IrRole::Assistant)
            .expect("the assistant turn must be present");

        assert_eq!(assistant.parts.len(), 2);

        #[expect(
            clippy::indexing_slicing,
            reason = "the two tool-call parts are asserted above"
        )]
        match &assistant.parts[0] {
            IrPart::ToolCall {
                name,
                thought_signature,
                ..
            } => {
                assert_eq!(name, "lint");
                assert_eq!(thought_signature.as_deref(), Some("sig-a"));
            }
            other => {
                #[expect(
                    clippy::panic,
                    reason = "a part of another kind fails the mapping under test"
                )]
                {
                    panic!("expected a tool call, got {other:?}")
                }
            }
        }
    }

    /// Verifies image parts map onto the existing IR images: `inlineData` becomes inline
    /// bytes and `fileData` with a public URL stays remote.
    ///
    /// # Panics
    ///
    /// Panics if the parsed images diverge from the documented mapping.
    #[test]
    fn maps_part_forms_onto_ir_images() {
        #[expect(clippy::expect_used, reason = "the image body is constructed to parse")]
        let parsed = parse_body(serde_json::json!({
            "contents": [
                {"role": "user", "parts": [
                    {"inlineData": {"mimeType": "image/png", "data": "aGVsbG8="}},
                    {"fileData": {"fileUri": "https://example.com/i.png"}},
                ]},
            ],
        }))
        .expect("image parts must parse");

        assert_eq!(parsed.ir.messages.len(), 1);

        #[expect(clippy::expect_used, reason = "the single user message is asserted")]
        let message = parsed
            .ir
            .messages
            .first()
            .expect("the user message must be present");

        assert_eq!(
            message.parts,
            vec![
                IrPart::Image {
                    image: IrImage::Inline {
                        mime_type: "image/png".to_string(),
                        data_base64: "aGVsbG8=".to_string(),
                    },
                },
                IrPart::Image {
                    image: IrImage::Url("https://example.com/i.png".to_string()),
                },
            ]
        );
    }

    /// Verifies the tool-choice mapping: `AUTO`/`NONE` plus `ANY` with zero or one allowed
    /// function; multiple allowed functions and unknown modes are 400s naming `toolConfig`,
    /// and a config without tools is a 400 as well.
    ///
    /// # Panics
    ///
    /// Panics if the parsed choice or rejection diverges from the contract.
    #[test]
    fn maps_tool_config_modes() {
        let tools = serde_json::json!([
            {"functionDeclarations": [{"name": "lint"}, {"name": "test"}]},
        ]);

        for (mode, allowed, expected) in [
            ("AUTO", Vec::<String>::new(), IrToolChoice::Auto),
            ("NONE", Vec::<String>::new(), IrToolChoice::None),
            ("ANY", Vec::<String>::new(), IrToolChoice::Required),
        ] {
            #[expect(clippy::expect_used, reason = "each mode is constructed to parse")]
            let parsed = parse_body(serde_json::json!({
                "contents": user_contents(),
                "tools": tools,
                "toolConfig": {
                    "functionCallingConfig": {"mode": mode, "allowedFunctionNames": allowed},
                },
            }))
            .expect("a valid toolConfig must parse");

            assert_eq!(parsed.ir.tool_choice, Some(expected));
        }

        #[expect(
            clippy::expect_used,
            reason = "the single-function ANY is constructed to parse"
        )]
        let parsed = parse_body(serde_json::json!({
            "contents": user_contents(),
            "tools": tools,
            "toolConfig": {
                "functionCallingConfig": {"mode": "ANY", "allowedFunctionNames": ["lint"]},
            },
        }))
        .expect("ANY with one allowed function must parse");

        assert_eq!(
            parsed.ir.tool_choice,
            Some(IrToolChoice::Function {
                name: "lint".to_string(),
            })
        );

        for body in [
            serde_json::json!({
                "contents": user_contents(),
                "tools": tools,
                "toolConfig": {
                    "functionCallingConfig": {
                        "mode": "ANY",
                        "allowedFunctionNames": ["lint", "test"],
                    },
                },
            }),
            serde_json::json!({
                "contents": user_contents(),
                "tools": tools,
                "toolConfig": {"functionCallingConfig": {"mode": "VALIDATED"}},
            }),
            serde_json::json!({
                "contents": user_contents(),
                "toolConfig": {"functionCallingConfig": {"mode": "AUTO"}},
            }),
        ] {
            #[expect(clippy::expect_used, reason = "each toolConfig is constructed to fail")]
            let error = parse_body(body).expect_err("an invalid toolConfig must fail");

            assert_eq!(error.param.as_deref(), Some("toolConfig"));
        }
    }

    /// Verifies built-in/server-side tools are 400s: a `google_search` entry never reaches
    /// the IR.
    ///
    /// # Panics
    ///
    /// Panics if the rejection lands outside a 400.
    #[test]
    fn rejects_builtin_tools() {
        #[expect(
            clippy::expect_used,
            reason = "the builtin tool is constructed to fail"
        )]
        let error = parse_body(serde_json::json!({
            "contents": user_contents(),
            "tools": [{"googleSearch": {}}],
        }))
        .expect_err("a built-in tool must fail");

        assert!(
            error.to_string().contains("unknown field"),
            "unexpected rejection: {error}"
        );
    }

    /// Verifies the generation-config mapping: sampling passes through, `thinkingLevel`
    /// parses the closed vocabulary (unknown is a 400 naming the field), `thinkingBudget`
    /// is a 400 naming the field wherever it appears, and stop/seed/media controls are
    /// 400s.
    ///
    /// # Panics
    ///
    /// Panics if the parsed config or rejection diverges from the contract.
    #[test]
    fn maps_generation_config() {
        #[expect(
            clippy::expect_used,
            reason = "the config body is constructed to parse"
        )]
        let parsed = parse_body(serde_json::json!({
            "contents": user_contents(),
            "generationConfig": {
                "temperature": 0.5,
                "topP": 0.9,
                "maxOutputTokens": 128,
                "thinkingConfig": {"thinkingLevel": "high"},
                "responseMimeType": "application/json",
            },
        }))
        .expect("a valid generationConfig must parse");

        assert_eq!(parsed.ir.temperature, Some(0.5));
        assert_eq!(parsed.ir.top_p, Some(0.9));
        assert_eq!(parsed.ir.max_output_tokens, Some(128));
        assert_eq!(parsed.ir.reasoning_effort, Some(IrReasoningLevel::High));
        assert_eq!(parsed.ir.response_format, Some(IrResponseFormat::Json));

        #[expect(
            clippy::expect_used,
            reason = "the unknown level is constructed to fail"
        )]
        let error = parse_body(serde_json::json!({
            "contents": user_contents(),
            "generationConfig": {"thinkingConfig": {"thinkingLevel": "ultra"}},
        }))
        .expect_err("an unknown thinkingLevel must fail");

        assert_eq!(error.param.as_deref(), Some("thinkingLevel"));

        for (field, body) in [
            (
                "thinkingBudget",
                serde_json::json!({
                    "contents": user_contents(),
                    "generationConfig": {"thinkingConfig": {"thinkingBudget": 128}},
                }),
            ),
            (
                "thinkingBudget",
                serde_json::json!({
                    "contents": user_contents(),
                    "generationConfig": {"thinkingBudget": 128},
                }),
            ),
            (
                "stopSequences",
                serde_json::json!({
                    "contents": user_contents(),
                    "generationConfig": {"stopSequences": ["end"]},
                }),
            ),
            (
                "seed",
                serde_json::json!({
                    "contents": user_contents(),
                    "generationConfig": {"seed": 7},
                }),
            ),
        ] {
            #[expect(clippy::expect_used, reason = "each control is constructed to fail")]
            let error = parse_body(body).expect_err("an unsupported control must fail");

            assert_eq!(error.param.as_deref(), Some(field));
        }
    }

    /// Verifies the response-format mapping: JSON without a schema is `Json`, with a schema
    /// is `JsonSchema` (synthesizing the IR name), and other MIME types are 400s.
    ///
    /// # Panics
    ///
    /// Panics if the parsed format diverges from the contract.
    #[test]
    fn maps_response_format() {
        #[expect(
            clippy::expect_used,
            reason = "the schema body is constructed to parse"
        )]
        let parsed = parse_body(serde_json::json!({
            "contents": user_contents(),
            "generationConfig": {
                "responseMimeType": "application/json",
                "responseSchema": {"type": "object"},
            },
        }))
        .expect("a schema response must parse");

        assert_eq!(
            parsed.ir.response_format,
            Some(IrResponseFormat::JsonSchema {
                name: "response".to_string(),
                schema: serde_json::json!({"type": "object"}),
            })
        );

        #[expect(
            clippy::expect_used,
            reason = "the unsupported MIME is constructed to fail"
        )]
        let error = parse_body(serde_json::json!({
            "contents": user_contents(),
            "generationConfig": {"responseMimeType": "image/png"},
        }))
        .expect_err("an unsupported MIME type must fail");

        assert_eq!(error.param.as_deref(), Some("responseMimeType"));
    }

    /// Verifies image content inside tool results is a 400 naming the result: a `user`
    /// content mixing `functionResponse` with image parts cannot enter the IR, which has
    /// no media channel on tool outputs.
    ///
    /// # Panics
    ///
    /// Panics if the rejection lands outside its documented wording.
    #[test]
    fn rejects_images_inside_tool_results() {
        #[expect(
            clippy::expect_used,
            reason = "the mixed result is constructed to fail"
        )]
        let error = parse_body(serde_json::json!({
            "contents": [
                {"role": "user", "parts": [
                    {"functionResponse": {"name": "lint", "response": {"ok": true}}},
                    {"inlineData": {"mimeType": "image/png", "data": "aGVsbG8="}},
                ]},
            ],
        }))
        .expect_err("an image inside a tool result must fail");

        assert_eq!(error.param.as_deref(), Some("functionResponse"));
    }

    /// Verifies unknown fields are 400s carrying serde's member name.
    ///
    /// # Panics
    ///
    /// Panics if a rejection lands outside its documented wording.
    #[test]
    fn rejects_unknown_fields() {
        #[expect(
            clippy::expect_used,
            reason = "the unknown field is constructed to fail"
        )]
        let error = parse_body(serde_json::json!({
            "contents": user_contents(),
            "bogus": 1,
        }))
        .expect_err("an unknown field must fail");

        assert!(
            error.to_string().contains("unknown field `bogus`"),
            "unexpected rejection: {error}"
        );
    }

    /// Verifies aggregated rendering emits the native shape: one `model` candidate with
    /// text plus function calls, a mapped finish reason, and Gemini usage columns.
    ///
    /// # Panics
    ///
    /// Panics if the rendered body diverges from the native shape.
    #[test]
    fn renders_aggregated_native_shape() {
        use crate::ir::{IrAggregatedToolCall, IrUsage};

        let aggregation = IrAggregation {
            text: "hi".to_string(),
            reasoning: String::new(),
            reasoning_blocks: Vec::new(),
            tool_calls: vec![IrAggregatedToolCall {
                call_id: "call_1".to_string(),
                name: "lint".to_string(),
                arguments: "{}".to_string(),
                thought_signature: Some("sig".to_string()),
            }],
            finish: Some(IrFinishReason::ToolCall),
            usage: Some(IrUsage {
                input_tokens: 10,
                output_tokens: 6,
                cached_tokens: Some(2),
                cache_write_tokens: None,
                reasoning_tokens: Some(1),
            }),
        };

        let rendered = render_json_response("m", &aggregation);

        #[expect(clippy::expect_used, reason = "the rendered body always serializes")]
        let value = serde_json::to_value(&rendered).expect("the body serializes");

        assert_eq!(
            value.pointer("/candidates/0/content/role"),
            Some(&serde_json::json!("model"))
        );
        assert_eq!(
            value.pointer("/candidates/0/finishReason"),
            Some(&serde_json::json!("STOP"))
        );
        assert_eq!(
            value.pointer("/candidates/0/content/parts/0/text"),
            Some(&serde_json::json!("hi"))
        );
        assert_eq!(
            value.pointer("/candidates/0/content/parts/1/functionCall/name"),
            Some(&serde_json::json!("lint"))
        );
        assert_eq!(
            value.pointer("/usageMetadata/promptTokenCount"),
            Some(&serde_json::json!(10))
        );
        assert_eq!(
            value.pointer("/usageMetadata/candidatesTokenCount"),
            Some(&serde_json::json!(5))
        );
        assert_eq!(
            value.pointer("/usageMetadata/thoughtsTokenCount"),
            Some(&serde_json::json!(1))
        );
        assert_eq!(
            value.pointer("/modelVersion"),
            Some(&serde_json::json!("m"))
        );
    }

    /// Verifies the streaming renderer delivers text live and renders a done-only
    /// reasoning block exactly once: a non-empty done with no prior deltas emits its
    /// thought part, while a done after deltas emits nothing.
    ///
    /// # Panics
    ///
    /// Panics if the rendered frames diverge from the documented behavior.
    #[test]
    fn streams_deltas_live_and_done_only_reasoning_once() {
        let mut renderer = SseRenderer::new("m".to_string());

        let text = renderer.render(&IrEvent::TextDelta("hi".to_string()));

        assert_eq!(text.len(), 1);
        #[expect(
            clippy::indexing_slicing,
            reason = "the single rendered frame is asserted above"
        )]
        let first_frame = &text[0];
        assert!(first_frame.contains(r#""text":"hi""#));

        let mut reasoning_only = SseRenderer::new("m".to_string());

        let done = reasoning_only.render(&IrEvent::ReasoningDone {
            text: "thought".to_string(),
            signature: None,
            opaque: Value::Null,
        });

        assert_eq!(done.len(), 1);
        #[expect(
            clippy::indexing_slicing,
            reason = "the single rendered frame is asserted above"
        )]
        let done_frame = &done[0];
        assert!(done_frame.contains(r#""thought":true"#));

        let mut with_deltas = SseRenderer::new("m".to_string());

        with_deltas.render(&IrEvent::ReasoningDelta("a".to_string()));

        let done = with_deltas.render(&IrEvent::ReasoningDone {
            text: "a".to_string(),
            signature: None,
            opaque: Value::Null,
        });

        assert!(done.is_empty());
    }
}
