//! Gemini GenerateContent protocol codec.
//!
//! Speaks the `gemini_generate_content` upstream protocol, both in the Antigravity Code Assist
//! envelope (§5.2) and as a bare OpenAI-compatible upstream protocol (§5.3).
//!
//! Rendered payloads are first-party types in the upstream wire shape, so field presence per
//! request is declared by `skip_serializing_if` rather than assembled conditionally.

use super::{PayloadError, UpstreamError, UpstreamEventParser};
use crate::{
    format, ids,
    ir::{
        InvalidRequest, IrEvent, IrFinishReason, IrImage, IrPart, IrRequest, IrResponseFormat,
        IrRole, IrToolChoice, IrUsage,
    },
    sse::SseFrame,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The `responseMimeType` value requesting a JSON response.
const JSON_MIME_TYPE: &str = "application/json";

/// Renders the upstream request URL and payload for an Antigravity provider.
///
/// `base_url` and `user_agent` are the provider's Code Assist endpoint and the `User-Agent` value
/// it requires; the provider layer owns endpoint knowledge, so both are threaded in rather than
/// owned by the codec.
///
/// # Errors
///
/// Returns [`InvalidRequest`] when the request cannot be rendered.
pub(crate) fn render_antigravity_request<'a>(
    ir: &'a IrRequest,
    base_url: &str,
    user_agent: &'a str,
    upstream_id: &'a str,
    project: &'a str,
    is_streaming: bool,
) -> Result<(String, AntigravityEnvelope<'a>), InvalidRequest> {
    let url = render_request_url(base_url, upstream_id, is_streaming, true);

    let envelope = AntigravityEnvelope {
        project,
        model: upstream_id,
        request: render_generate_content_request(ir)?,
        request_type: "agent",
        user_agent,
        request_id: format!("agent-{}", ids::new_uuid()),
    };

    Ok((url, envelope))
}

/// The Code Assist envelope wrapping one GenerateContentRequest (README §5.2).
#[derive(Debug, Serialize)]
pub(crate) struct AntigravityEnvelope<'a> {
    /// Cloud Code Assist project the request bills to.
    project: &'a str,

    /// Upstream model name.
    model: &'a str,

    /// The wrapped request.
    request: GenerateContentRequest<'a>,

    /// Always `agent`.
    #[serde(rename = "requestType")]
    request_type: &'static str,

    /// `User-Agent` value the Code Assist endpoint requires, echoed in the
    /// body.
    #[serde(rename = "userAgent")]
    user_agent: &'a str,

    /// Gateway-minted request identifier.
    #[serde(rename = "requestId")]
    request_id: String,
}

/// Renders the upstream request URL and payload for an OpenAI-compatible Gemini provider, whose
/// body is the bare GenerateContentRequest.
///
/// # Errors
///
/// Returns [`InvalidRequest`] when the request cannot be rendered.
pub(crate) fn render_compatible_request<'a>(
    ir: &'a IrRequest,
    base_url: &str,
    upstream_id: &str,
    is_streaming: bool,
) -> Result<(String, GenerateContentRequest<'a>), InvalidRequest> {
    let url = render_request_url(base_url, upstream_id, is_streaming, false);

    Ok((url, render_generate_content_request(ir)?))
}

/// Renders the GenerateContent URL for one call, the single place where the action segment
/// (including its leading colon) is assembled: enveloped Code Assist upstreams take the action
/// directly on the base URL, compatible upstreams insert `/models/<upstream_id>` before it.
fn render_request_url(
    base_url: &str,
    upstream_id: &str,
    is_streaming: bool,
    is_enveloped: bool,
) -> String {
    /// Action segment selecting plain generation.
    const GENERATE_ACTION: &str = ":generateContent";

    /// Action segment selecting SSE streaming generation.
    const STREAM_GENERATE_ACTION: &str = ":streamGenerateContent?alt=sse";

    let base_url = base_url.trim_end_matches('/');

    let action = if is_streaming {
        STREAM_GENERATE_ACTION
    } else {
        GENERATE_ACTION
    };

    if is_enveloped {
        format!("{base_url}{action}")
    } else {
        format!("{base_url}/models/{upstream_id}{action}")
    }
}

/// The bare GenerateContentRequest carried in the envelope `request` field or sent directly to a
/// compatible upstream.
#[derive(Debug, Serialize)]
pub(crate) struct GenerateContentRequest<'a> {
    /// Conversation contents, in alternating-role runs.
    contents: Vec<Content<'a>>,

    /// Concatenated system instructions, omitted when the request has none.
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    system_instruction: Option<SystemInstruction<'a>>,

    /// Advertised function tools, omitted when the request has none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolGroup<'a>>,

    /// Tool selection restriction, omitted when the request has none.
    #[serde(rename = "toolConfig", skip_serializing_if = "Option::is_none")]
    tool_config: Option<ToolConfig<'a>>,

    /// Sampling and output configuration, omitted when it would be empty.
    #[serde(rename = "generationConfig", skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig<'a>>,
}

/// One content entry: the parts of an alternating-role run of messages.
#[derive(Debug, Serialize)]
struct Content<'a> {
    /// Gemini content role, `user` or `model`.
    role: &'static str,

    /// The run's parts in order.
    parts: Vec<Part<'a>>,
}

/// The system instruction.
#[derive(Debug, Serialize)]
struct SystemInstruction<'a> {
    /// Instruction parts; the gateway renders exactly one text part.
    parts: Vec<Part<'a>>,
}

/// One Gemini part: the union of the part kinds the gateway renders, exactly one per part.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Part<'a> {
    /// Plain text.
    Text {
        /// The text content.
        text: &'a str,
    },

    /// Base64-encoded inline image data.
    InlineData {
        /// The inline data object.
        #[serde(rename = "inlineData")]
        inline_data: InlineData<'a>,
    },

    /// An image referenced by URL.
    FileData {
        /// The file data object.
        #[serde(rename = "fileData")]
        file_data: FileData<'a>,
    },

    /// An assistant tool invocation.
    FunctionCall {
        /// The invocation itself.
        #[serde(rename = "functionCall")]
        function_call: FunctionCall<'a>,

        /// Upstream thought signature that must be replayed on the next turn.
        #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
        thought_signature: Option<&'a str>,
    },

    /// A tool result for a prior function call.
    FunctionResponse {
        /// The result itself.
        #[serde(rename = "functionResponse")]
        function_response: FunctionResponse<'a>,
    },
}

/// Inline image data with its MIME type.
#[derive(Debug, Serialize)]
struct InlineData<'a> {
    /// MIME type of the image data.
    #[serde(rename = "mimeType")]
    mime_type: &'a str,

    /// Base64-encoded image bytes.
    data: &'a str,
}

/// An image referenced by URL.
#[derive(Debug, Serialize)]
struct FileData<'a> {
    /// The image URL.
    #[serde(rename = "fileUri")]
    file_uri: &'a str,
}

/// A tool invocation.
#[derive(Debug, Serialize)]
struct FunctionCall<'a> {
    /// Name of the invoked function.
    name: &'a str,

    /// Arguments object, decoded from the IR's JSON-encoded arguments text.
    args: Map<String, Value>,

    /// Identifier of the invocation. The IR's client-populated `call_id` is exactly what the
    /// upstream's `FunctionResponse.id` is defined to match, so parallel calls to one function
    /// name stay resolvable.
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a str>,
}

/// A tool result.
#[derive(Debug, Serialize)]
struct FunctionResponse<'a> {
    /// Name of the function being answered.
    name: String,

    /// The result payload.
    response: ToolResult<'a>,

    /// Identifier of the invocation being answered (see
    /// [`FunctionCall::id`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a str>,
}

/// The result payload of a tool response: the tool's own JSON object when the stored output is one,
/// else the `{"result": ...}` wrapper the upstream requires.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ToolResult<'a> {
    /// A JSON-object result, forwarded verbatim.
    Object(Map<String, Value>),

    /// Any other parsed JSON value, wrapped.
    Wrapped {
        /// The wrapped value, whose shape is the tool's own.
        result: Value,
    },

    /// A result that is not JSON at all, wrapped as text.
    Text {
        /// The wrapped text.
        result: &'a str,
    },
}

/// One entry of the upstream `tools` array: a declaration group.
#[derive(Debug, Serialize)]
struct ToolGroup<'a> {
    /// The advertised function declarations.
    #[serde(rename = "functionDeclarations")]
    function_declarations: Vec<FunctionDeclaration<'a>>,
}

/// One advertised function declaration.
#[derive(Debug, Serialize)]
struct FunctionDeclaration<'a> {
    /// Function name the model calls.
    name: &'a str,

    /// Human-readable description.
    description: Option<&'a str>,

    /// JSON Schema of the accepted arguments, absent for a function that
    /// takes none.
    parameters: Option<&'a Value>,
}

/// The tool selection configuration.
#[derive(Debug, Serialize)]
struct ToolConfig<'a> {
    /// The function calling configuration.
    #[serde(rename = "functionCallingConfig")]
    function_calling_config: FunctionCallingConfig<'a>,
}

/// The function calling configuration.
#[derive(Debug, Serialize)]
struct FunctionCallingConfig<'a> {
    /// Selection mode.
    mode: FunctionCallingMode,

    /// Functions the model may call under the `ANY` mode, omitted when the
    /// mode does not restrict the choice.
    #[serde(rename = "allowedFunctionNames", skip_serializing_if = "Vec::is_empty")]
    allowed_function_names: Vec<&'a str>,
}

/// The Gemini `functionCallingConfig.mode` values.
#[derive(Debug, Clone, Copy, Serialize)]
enum FunctionCallingMode {
    /// The model decides freely.
    #[serde(rename = "AUTO")]
    Auto,

    /// The model may not call tools.
    #[serde(rename = "NONE")]
    None,

    /// The model must call a tool.
    #[serde(rename = "ANY")]
    Required,
}

/// Sampling and output configuration.
#[derive(Debug, Serialize)]
struct GenerationConfig<'a> {
    /// Sampling temperature, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,

    /// Nucleus sampling threshold, omitted when the request has none.
    #[serde(rename = "topP", skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,

    /// Output token budget, omitted when the request has none.
    #[serde(rename = "maxOutputTokens", skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u64>,

    /// Response MIME type, present exactly when the request constrains the
    /// response format.
    #[serde(rename = "responseMimeType", skip_serializing_if = "Option::is_none")]
    response_mime_type: Option<&'static str>,

    /// JSON Schema the response must conform to; the schema's shape is the
    /// client's to define.
    #[serde(rename = "responseSchema", skip_serializing_if = "Option::is_none")]
    response_schema: Option<&'a Value>,

    /// Thinking configuration, omitted when the request named no level.
    #[serde(rename = "thinkingConfig", skip_serializing_if = "Option::is_none")]
    thinking_config: Option<ThinkingConfig>,
}

impl GenerationConfig<'_> {
    /// Whether every field is absent, in which case the whole configuration
    /// is omitted from the request.
    fn is_empty(&self) -> bool {
        self.temperature.is_none()
            && self.top_p.is_none()
            && self.max_output_tokens.is_none()
            && self.response_mime_type.is_none()
            && self.response_schema.is_none()
            && self.thinking_config.is_none()
    }
}

/// Thinking configuration.
#[derive(Debug, Serialize)]
struct ThinkingConfig {
    /// Token budget rendered from the canonical thinking level (README §6.4).
    #[serde(rename = "thinkingBudget")]
    thinking_budget: u32,
}

/// Renders the bare GenerateContentRequest carried in the envelope `request` field or sent directly
/// to a compatible upstream.
///
/// The Gemini API requires `contents` roles to alternate between `user` and `model`, so each run of
/// consecutive same-role messages coalesces into one content entry whose parts concatenate in order
/// — the several tool results of one turn then ride as multiple `functionResponse` parts of a
/// single `user` content, the canonical shape. An already-alternating history renders one content
/// per message, exactly the 1:1 shape.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for tool call arguments that are not JSON objects and tool outputs
/// whose invocation cannot be resolved.
fn render_generate_content_request(
    ir: &IrRequest,
) -> Result<GenerateContentRequest<'_>, InvalidRequest> {
    let mut contents: Vec<Content<'_>> = Vec::new();

    // The open run's role and accumulated parts; a message whose parts render to nothing is
    // transparent — it contributes nothing and cannot split a run, since it produces no content
    // entry to alternate against.
    let mut run_role: Option<&'static str> = None;
    let mut run_parts: Vec<Part<'_>> = Vec::new();

    for message in &ir.messages {
        let role = match message.role {
            IrRole::User | IrRole::Tool => "user",
            IrRole::Assistant => "model",
        };

        let parts: Vec<Part<'_>> = message
            .parts
            .iter()
            .map(|part| render_part(part, ir))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect();

        if parts.is_empty() {
            continue;
        }

        if run_role != Some(role)
            && let Some(previous_role) = run_role.replace(role)
        {
            contents.push(Content {
                role: previous_role,
                parts: run_parts,
            });

            run_parts = Vec::new();
        }

        run_parts.extend(parts);
    }

    if let Some(previous_role) = run_role {
        contents.push(Content {
            role: previous_role,
            parts: run_parts,
        });
    }

    let system_instruction = ir
        .instructions
        .as_ref()
        .map(|instructions| SystemInstruction {
            parts: vec![Part::Text { text: instructions }],
        });

    let tools = if ir.tools.is_empty() {
        Vec::new()
    } else {
        vec![ToolGroup {
            function_declarations: ir
                .tools
                .iter()
                .map(|tool| FunctionDeclaration {
                    name: &tool.name,
                    description: tool.description.as_deref(),
                    parameters: tool.parameters.as_ref(),
                })
                .collect(),
        }]
    };

    let tool_config = ir.tool_choice.as_ref().map(|tool_choice| ToolConfig {
        function_calling_config: match tool_choice {
            IrToolChoice::Auto => FunctionCallingConfig {
                mode: FunctionCallingMode::Auto,
                allowed_function_names: Vec::new(),
            },

            IrToolChoice::None => FunctionCallingConfig {
                mode: FunctionCallingMode::None,
                allowed_function_names: Vec::new(),
            },

            IrToolChoice::Required => FunctionCallingConfig {
                mode: FunctionCallingMode::Required,
                allowed_function_names: Vec::new(),
            },

            IrToolChoice::Function { name } => FunctionCallingConfig {
                mode: FunctionCallingMode::Required,
                allowed_function_names: vec![name],
            },
        },
    });

    let (response_mime_type, response_schema) = match &ir.response_format {
        Some(IrResponseFormat::Json) => (Some(JSON_MIME_TYPE), None),

        Some(IrResponseFormat::JsonSchema { schema, .. }) => (Some(JSON_MIME_TYPE), Some(schema)),

        _ => (None, None),
    };

    let generation_config = GenerationConfig {
        temperature: ir.temperature,
        top_p: ir.top_p,
        max_output_tokens: ir.max_output_tokens,
        response_mime_type,
        response_schema,
        thinking_config: ir.reasoning_effort.map(|effort| ThinkingConfig {
            thinking_budget: effort.to_thinking_budget(),
        }),
    };

    Ok(GenerateContentRequest {
        contents,
        system_instruction,
        tools,
        tool_config,
        generation_config: (!generation_config.is_empty()).then_some(generation_config),
    })
}

/// Resolves the function name a tool output answers, scanning earlier messages for the matching
/// invocation.
///
/// # Errors
///
/// Returns [`InvalidRequest`] when no matching tool call exists in the conversation.
fn resolve_tool_output_name(call_id: &str, ir: &IrRequest) -> Result<String, InvalidRequest> {
    for message in &ir.messages {
        for part in &message.parts {
            if let IrPart::ToolCall {
                call_id: candidate,
                name,
                ..
            } = part
                && candidate == call_id
            {
                return Ok(name.clone());
            }
        }
    }

    Err(InvalidRequest::field(
        "messages",
        format!("Tool output references unknown call_id `{call_id}`"),
    ))
}

/// Renders one IR part into zero or more Gemini parts.
///
/// # Errors
///
/// Returns [`InvalidRequest`] per [`resolve_tool_output_name`] and for non-object tool call
/// arguments.
fn render_part<'a>(part: &'a IrPart, ir: &IrRequest) -> Result<Vec<Part<'a>>, InvalidRequest> {
    match part {
        IrPart::Text { text } => Ok(vec![Part::Text { text }]),

        IrPart::Image { image } => Ok(vec![match image {
            IrImage::Inline {
                mime_type,
                data_base64,
            } => Part::InlineData {
                inline_data: InlineData {
                    mime_type,
                    data: data_base64,
                },
            },

            IrImage::Url(url) => Part::FileData {
                file_data: FileData { file_uri: url },
            },
        }]),

        IrPart::ToolCall {
            call_id,
            name,
            arguments,
            thought_signature,
        } => Ok(vec![Part::FunctionCall {
            function_call: FunctionCall {
                name,
                args: format::arguments_as_object(arguments)?,
                id: Some(call_id),
            },
            thought_signature: thought_signature.as_deref(),
        }]),

        IrPart::ToolOutput { call_id, content } => {
            let name = resolve_tool_output_name(call_id, ir)?;

            let response = match serde_json::from_str::<Value>(content) {
                Ok(Value::Object(map)) => ToolResult::Object(map),
                Ok(other) => ToolResult::Wrapped { result: other },
                Err(_) => ToolResult::Text { result: content },
            };

            Ok(vec![Part::FunctionResponse {
                function_response: FunctionResponse {
                    name,
                    response,
                    id: Some(call_id),
                },
            }])
        }
    }
}

/// Parses upstream Gemini responses — SSE frames or a full JSON body — into IR events.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Count of function calls synthesized in this response.
    call_counter: usize,

    /// Whether the response delivered any `functionCall` part; Gemini ends tool-call turns with
    /// the plain `STOP` reason, so the delivered calls decide the canonical tool-call finish.
    is_call_delivered: bool,

    /// Whether a finish reason was already emitted; a clean end-of-stream without one degrades to
    /// [`IrFinishReason::Stop`] in the parser's finish hook, matching the aggregated rendering
    /// default.
    is_finished: bool,
}

/// One upstream GenerateContentResponse, or the Cloud Code Assist envelope wrapping one.
///
/// The Cloud Code Assist API wraps every GenerateContentResponse — `:generateContent` JSON bodies
/// and `alt=sse` frames alike — in a `response` member beside `traceId` and `metadata` (README
/// §5.2), while Gemini-compatible upstreams carry the bare response, which has no top-level
/// `response` member of its own. Every member is optional because the body is either the envelope
/// or the bare response, and upstreams are third-party: members the gateway does not read are
/// ignored.
#[derive(Debug, Default, Deserialize)]
struct UpstreamBody {
    /// The response wrapped by the Code Assist envelope.
    #[serde(default)]
    response: Option<Box<UpstreamBody>>,

    /// Failure the upstream may embed in a 2xx body (README §6.7).
    #[serde(default)]
    error: Option<UpstreamError>,

    /// Upstream response identifier.
    #[serde(rename = "responseId", default)]
    response_id: Option<String>,

    /// Upstream model name, empty when the upstream omits it.
    #[serde(rename = "modelVersion", default)]
    model_version: Option<String>,

    /// Candidate answers; the gateway reads the primary one.
    #[serde(default)]
    candidates: Vec<Candidate>,

    /// Prompt-level feedback, sent only when no candidate was generated.
    #[serde(rename = "promptFeedback", default)]
    prompt_feedback: Option<PromptFeedback>,

    /// Token accounting.
    #[serde(rename = "usageMetadata", default)]
    usage_metadata: Option<UsageMetadata>,
}

impl UpstreamBody {
    /// The GenerateContentResponse to ingest: the envelope's wrapped response when the body is an
    /// envelope, else the body itself.
    fn payload(&self) -> &Self {
        match &self.response {
            Some(payload) => payload,

            None => self,
        }
    }
}

/// One upstream candidate answer.
#[derive(Debug, Default, Deserialize)]
struct Candidate {
    /// Upstream-assigned index, absent on upstreams that omit it.
    #[serde(default)]
    index: Option<u64>,

    /// The candidate's content.
    #[serde(default)]
    content: Option<CandidateContent>,

    /// Upstream finish reason string.
    #[serde(rename = "finishReason", default)]
    finish_reason: Option<String>,
}

/// The content of one upstream candidate.
#[derive(Debug, Default, Deserialize)]
struct CandidateContent {
    /// The content parts in order.
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

/// One upstream content part, carrying its text, its tool call, or neither.
#[derive(Debug, Default, Deserialize)]
struct GeminiPart {
    /// Text content, absent on a non-text part.
    #[serde(default)]
    text: Option<String>,

    /// Whether the text is reasoning rather than answer text.
    #[serde(default)]
    thought: Option<bool>,

    /// The tool call, present on a `functionCall` part.
    #[serde(rename = "functionCall", default)]
    function_call: Option<FunctionCallPayload>,

    /// Upstream thought signature that must be replayed on the next turn.
    #[serde(rename = "thoughtSignature", default)]
    thought_signature: Option<String>,
}

/// The tool call of one upstream part.
#[derive(Debug, Default, Deserialize)]
struct FunctionCallPayload {
    /// Name of the invoked function, empty when the upstream omits it.
    #[serde(default)]
    name: Option<String>,

    /// Arguments the model supplied; the object's members are the tool's own schema, which the
    /// gateway forwards verbatim.
    #[serde(default)]
    args: Option<Map<String, Value>>,

    /// Invocation identifier, absent on an upstream that does not mint one.
    #[serde(default)]
    id: Option<String>,
}

/// Prompt-level feedback of a blocked prompt.
#[derive(Debug, Default, Deserialize)]
struct PromptFeedback {
    /// Why the prompt was blocked.
    #[serde(rename = "blockReason", default)]
    block_reason: Option<String>,
}

/// Upstream token accounting in the Gemini shape.
#[derive(Debug, Default, Deserialize)]
struct UsageMetadata {
    /// Tokens consumed by the prompt.
    #[serde(rename = "promptTokenCount", default)]
    prompt_token_count: u64,

    /// Tokens produced by the candidates, excluding reasoning.
    #[serde(rename = "candidatesTokenCount", default)]
    candidates_token_count: u64,

    /// Tokens spent on model reasoning.
    #[serde(rename = "thoughtsTokenCount", default)]
    thoughts_token_count: Option<u64>,

    /// Prompt tokens served from the upstream cache.
    #[serde(rename = "cachedContentTokenCount", default)]
    cached_content_token_count: Option<u64>,
}

impl UpstreamParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ingests one GenerateContentResponse object.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for an embedded upstream error object and for a
    /// `MALFORMED_FUNCTION_CALL` finish.
    fn ingest_response(&mut self, body: UpstreamBody) -> Result<Vec<IrEvent>, PayloadError> {
        // An error object inside a 2xx body is a malformed upstream payload (README §6.7); the
        // check sits beside the protocol fields, before the `response` envelope below is unwrapped,
        // shared with the OpenAI-family codecs.
        if let Some(error) = &body.error {
            return Err(error.failure());
        }

        let value = body.payload();

        let mut events = Vec::new();

        if !self.is_started {
            self.is_started = true;

            events.push(IrEvent::MessageStart {
                id: value
                    .response_id
                    .clone()
                    .unwrap_or_else(|| format!("gen-{}", ids::new_uuid())),
                model: value.model_version.clone().unwrap_or_default(),
            });
        }

        if let Some(candidate) = primary_candidate(&value.candidates) {
            if let Some(content) = &candidate.content {
                for part in &content.parts {
                    self.ingest_part(part, &mut events);
                }
            }

            if let Some(finish_reason) = &candidate.finish_reason {
                self.is_finished = true;

                events.push(IrEvent::Finish(finish_reason_from_gemini(
                    finish_reason,
                    self.is_call_delivered,
                )?));
            }
        }

        // A prompt-level block carries `promptFeedback.blockReason` and no candidates, so no
        // `finishReason` ever arrives and the clean end of stream would degrade the block into a
        // successful `Stop`. Upstream sends `promptFeedback` only when no candidate was generated,
        // so a candidate finish above always wins.
        if !self.is_finished
            && value
                .prompt_feedback
                .as_ref()
                .and_then(|feedback| feedback.block_reason.as_deref())
                .is_some()
        {
            self.is_finished = true;

            events.push(IrEvent::Finish(IrFinishReason::ContentFilter));
        }

        if let Some(usage) = &value.usage_metadata {
            let reasoning_tokens = usage.thoughts_token_count;

            events.push(IrEvent::Usage(IrUsage {
                input_tokens: usage.prompt_token_count,
                output_tokens: usage.candidates_token_count + reasoning_tokens.unwrap_or(0),
                cached_tokens: usage.cached_content_token_count,
                // Gemini reports no cache-write accounting.
                cache_write_tokens: None,
                reasoning_tokens,
            }));
        }

        Ok(events)
    }

    /// Ingests one content part of the primary candidate.
    ///
    /// # Panics
    ///
    /// Panics if serializing the borrowed `args` map fails, which cannot occur: serializing a JSON
    /// object map is infallible.
    fn ingest_part(&mut self, part: &GeminiPart, events: &mut Vec<IrEvent>) {
        if let Some(text) = &part.text {
            let is_thought = part.thought.unwrap_or(false);

            events.push(if is_thought {
                IrEvent::ReasoningDelta(text.clone())
            } else {
                IrEvent::TextDelta(text.clone())
            });
        }

        let Some(function_call) = &part.function_call else {
            return;
        };

        self.is_call_delivered = true;

        let name = function_call.name.clone().unwrap_or_default();

        // The arguments serialize straight from the borrowed map — no deep copy — with a missing
        // `args` rendering an empty JSON object.
        let arguments = match &function_call.args {
            Some(args) =>
            {
                #[expect(
                    clippy::expect_used,
                    reason = "serializing a JSON object map cannot fail"
                )]
                serde_json::to_string(args).expect("JSON map serialization is infallible")
            }

            None => "{}".to_string(),
        };

        // A populated upstream `functionCall.id` is the cross-turn matching channel: the replayed
        // `functionResponse.id` must answer it, and without it two same-name calls in one turn are
        // unresolvable. An id-less call mints one so the pairing channel always exists; the minted
        // id also rides outward as the client-facing tool-call id.
        let call_id = match function_call.id.clone() {
            Some(id) if !id.is_empty() => id,
            _ => format!("call_{}", ids::new_uuid()),
        };

        events.push(IrEvent::ToolCallDelta {
            index: self.call_counter,
            call_id: Some(call_id.clone()),
            name: Some(name.clone()),
            arguments_delta: arguments.clone(),
        });

        events.push(IrEvent::ToolCallDone {
            index: self.call_counter,
            call_id,
            name,
            arguments,
            thought_signature: part.thought_signature.clone(),
        });

        self.call_counter += 1;
    }
}

impl UpstreamEventParser for UpstreamParser {
    /// Ingests one upstream SSE frame carrying a GenerateContentResponse.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for malformed JSON and upstream error objects.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, PayloadError> {
        let body = format::parse_frame::<UpstreamBody>(frame)?;

        self.ingest_response(body)
    }

    /// Parses a complete upstream non-streaming JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for upstream error objects and malformed bodies.
    fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, PayloadError> {
        let body = format::parse_body::<UpstreamBody>(value)?;

        self.ingest_response(body)
    }

    fn finish(&mut self) -> Vec<IrEvent> {
        // An upstream may close its stream cleanly after streaming content without ever sending a
        // `finishReason`; the missing finish reason degrades to `Stop` so the streamed client sees
        // the same terminal finish reason as the aggregated rendering of the same stream.
        if !self.is_finished {
            self.is_finished = true;

            return vec![IrEvent::Finish(IrFinishReason::Stop)];
        }

        Vec::new()
    }
}

/// Selects the primary candidate of an upstream `candidates` array: the entry whose `index` is 0,
/// tolerating index-less upstreams by taking the first entry instead. Parallel candidates beyond
/// the primary are not supported and the remainder is ignored.
fn primary_candidate(candidates: &[Candidate]) -> Option<&Candidate> {
    candidates
        .iter()
        .find(|candidate| candidate.index == Some(0))
        .or_else(|| candidates.first())
}

/// Maps a Gemini `finishReason` to the canonical reason.
///
/// Gemini ends function-call turns with the plain `STOP` reason — the `finishReason` vocabulary has
/// no tool-call value — so a turn that delivered `functionCall` parts carries the canonical
/// [`IrFinishReason::ToolCall`] finish, matching official Chat Completions semantics for
/// text+tool-call mixed turns too.
///
/// # Errors
///
/// Returns [`PayloadError`] for `MALFORMED_FUNCTION_CALL`: the model generated an invalid function
/// call, so the turn delivered no usable result and must surface as an upstream failure (README
/// §6.7) instead of a successful `Stop` finish that would mask the failure.
fn finish_reason_from_gemini(
    value: &str,
    is_call_delivered: bool,
) -> Result<IrFinishReason, PayloadError> {
    match value {
        "MAX_TOKENS" => Ok(IrFinishReason::Length),

        // The official `FinishReason` vocabulary flags filtered content with these six reasons —
        // `LANGUAGE` ("the candidate content was flagged for using an unsupported language")
        // grouped with its safety siblings — so each is a filtered turn, never a clean stop.
        "LANGUAGE" | "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => {
            Ok(IrFinishReason::ContentFilter)
        }

        "MALFORMED_FUNCTION_CALL" => Err(PayloadError::new(
            "upstream finish reason `MALFORMED_FUNCTION_CALL`: \
             the function call generated by the model is invalid",
        )),

        _ if is_call_delivered => Ok(IrFinishReason::ToolCall),

        _ => Ok(IrFinishReason::Stop),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::IrMessage;

    /// Renders a bare GenerateContentRequest into the JSON value the tests
    /// assert against.
    ///
    /// # Panics
    ///
    /// Panics if the rendered request fails to serialize.
    #[expect(
        clippy::expect_used,
        reason = "the rendered request is under test; a failing render is the failure"
    )]
    fn request_json(request: &GenerateContentRequest<'_>) -> Value {
        serde_json::to_value(request).expect("the rendered request serializes")
    }

    /// A turn whose upstream reported `MALFORMED_FUNCTION_CALL` is a failed
    /// generation, not a successful completion: both the JSON body and the
    /// streaming path reject it as an upstream failure (README §6.7) instead
    /// of rendering a clean finish that would mask the failure.
    ///
    /// # Panics
    ///
    /// Panics if the flagged turn parses as a success or its wording drifts.
    #[test]
    fn malformed_function_call_finish_reason_is_an_upstream_failure() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "partial"}]},
                "finishReason": "MALFORMED_FUNCTION_CALL",
            }],
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the flagged body is constructed to be rejected"
        )]
        let error = parser
            .parse_json_response(body)
            .expect_err("a malformed function call turn must fail");

        assert_eq!(
            error.to_string(),
            "upstream finish reason `MALFORMED_FUNCTION_CALL`: \
             the function call generated by the model is invalid",
        );

        let frame = SseFrame {
            data: serde_json::json!({"candidates": [{"finishReason": "MALFORMED_FUNCTION_CALL"}]})
                .to_string(),
        };

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the flagged frame is constructed to be rejected"
        )]
        let error = parser
            .ingest_frame(&frame)
            .expect_err("a malformed function call stream frame must fail");

        assert!(error.to_string().contains("MALFORMED_FUNCTION_CALL"));
    }

    /// Ordinary finish reasons keep mapping through the now fallible mapper:
    /// a turn cut off by the token limit still parses and reports the
    /// canonical length finish.
    ///
    /// # Panics
    ///
    /// Panics if the flagged turn fails to parse.
    #[test]
    fn ordinary_finish_reasons_still_map() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "partial"}]},
                "finishReason": "MAX_TOKENS",
            }],
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the MAX_TOKENS turn is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body)
            .expect("a MAX_TOKENS turn parses");

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::Length)));
    }

    /// A prompt-level block carries `promptFeedback.blockReason` and no
    /// candidates, so no `finishReason` ever arrives and the clean end of
    /// stream must not degrade the block into a successful `Stop` finish:
    /// both the aggregated body and the streaming path report the canonical
    /// content-filter finish.
    ///
    /// # Panics
    ///
    /// Panics if the blocked turn fails to parse or its finish drifts.
    #[test]
    fn prompt_block_finishes_as_content_filter() {
        let body = serde_json::json!({"promptFeedback": {"blockReason": "SAFETY"}});

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the prompt-blocked body is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body.clone())
            .expect("a prompt-blocked body parses");

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::ContentFilter)));
        assert!(parser.finish().is_empty(), "no spurious Stop may follow");

        let frame = SseFrame {
            data: body.to_string(),
        };

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the prompt-blocked frame is constructed to parse"
        )]
        let events = parser
            .ingest_frame(&frame)
            .expect("a prompt-blocked frame parses");

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::ContentFilter)));
    }

    /// A turn the upstream flagged for an unsupported language (`LANGUAGE`)
    /// belongs to the same flagged-content class as the safety siblings: both
    /// the aggregated body and the streaming path report the canonical
    /// content-filter finish instead of a clean `Stop` that would hide the
    /// filtering from clients branching on it.
    ///
    /// # Panics
    ///
    /// Panics if the flagged turn fails to parse or its finish drifts.
    #[test]
    fn language_finish_reason_maps_to_content_filter() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "partial"}]},
                "finishReason": "LANGUAGE",
            }],
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the LANGUAGE-flagged body is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body.clone())
            .expect("a LANGUAGE-flagged body parses");

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::ContentFilter)));
        assert!(parser.finish().is_empty(), "no spurious Stop may follow");

        let frame = SseFrame {
            data: body.to_string(),
        };

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the LANGUAGE-flagged frame is constructed to parse"
        )]
        let events = parser
            .ingest_frame(&frame)
            .expect("a LANGUAGE-flagged frame parses");

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::ContentFilter)));
    }

    /// Upstream sends `promptFeedback` only when no candidate was generated,
    /// so a response that also carried a finishing candidate keeps that
    /// candidate's finish reason instead of being overridden by the feedback.
    ///
    /// # Panics
    ///
    /// Panics if the finishing candidate fails to parse.
    #[test]
    fn candidate_finish_wins_over_prompt_feedback() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "ok"}]},
                "finishReason": "STOP",
            }],
            "promptFeedback": {"blockReason": "SAFETY"},
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the finishing candidate is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body)
            .expect("a finishing candidate parses");

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::Stop)));
        assert!(!events.contains(&IrEvent::Finish(IrFinishReason::ContentFilter)));
    }

    /// The `usageMetadata` breakdown fields flow into the IR usage detail:
    /// `cachedContentTokenCount` becomes the cached input count and
    /// `thoughtsTokenCount` the reasoning output count, while the output
    /// total keeps including the reasoning tokens and the absent
    /// cache-write count stays [`None`].
    ///
    /// # Panics
    ///
    /// Panics if the usage breakdown diverges from the upstream report.
    #[test]
    fn usage_breakdown_reports_cache_and_reasoning_tokens() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "hi"}]},
                "finishReason": "STOP",
            }],
            "usageMetadata": {
                "promptTokenCount": 100,
                "candidatesTokenCount": 20,
                "thoughtsTokenCount": 8,
                "cachedContentTokenCount": 60,
            },
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the usage-bearing body is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body)
            .expect("a usage-bearing body parses");

        let usage = events.iter().find_map(|event| match event {
            IrEvent::Usage(usage) => Some(*usage),
            _ => None,
        });

        assert_eq!(
            usage,
            Some(IrUsage {
                input_tokens: 100,
                output_tokens: 28,
                cached_tokens: Some(60),
                cache_write_tokens: None,
                reasoning_tokens: Some(8),
            })
        );
    }

    /// An error object inside a 2xx body is rejected through the shared
    /// OpenAI-family rejection, wording included; the shared classification
    /// also rejects the non-object error values the retired inline copy
    /// silently ignored (README §6.7 malformed-payload handling).
    ///
    /// # Panics
    ///
    /// Panics if an error body parses as a success or its wording drifts.
    #[test]
    fn embedded_error_object_uses_shared_rejection() {
        let body = serde_json::json!({
            "error": {"code": 400, "message": "API key not valid"},
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the embedded error object is constructed to be rejected"
        )]
        let error = parser
            .parse_json_response(body)
            .expect_err("an embedded error object must fail");

        assert_eq!(
            error.to_string(),
            "API key not valid (upstream error code: 400)"
        );

        let body = serde_json::json!({"error": "quota exhausted"});

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the non-object error value is constructed to be rejected"
        )]
        let error = parser
            .parse_json_response(body)
            .expect_err("a non-object error value must fail");

        assert_eq!(error.to_string(), "quota exhausted");
    }

    /// The Gemini API requires `contents` roles to alternate between `user`
    /// and `model`, so the two `functionResponse` parts of one tool turn —
    /// separate `IrRole::Tool` messages inbound — must land in a single `user`
    /// content instead of rendering consecutive `user` entries the upstream
    /// rejects with 400 INVALID_ARGUMENT. Every call and response part also
    /// carries its call id, the pairing channel the upstream matches on.
    ///
    /// # Panics
    ///
    /// Panics if the rendered contents diverge from the pinned wire shape.
    #[test]
    fn parallel_tool_results_land_in_one_user_content() {
        let ir = IrRequest {
            messages: vec![
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: "Run both checks".to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![
                        IrPart::ToolCall {
                            call_id: "call_a".to_string(),
                            name: "lint".to_string(),
                            arguments: "{}".to_string(),
                            thought_signature: None,
                        },
                        IrPart::ToolCall {
                            call_id: "call_b".to_string(),
                            name: "test".to_string(),
                            arguments: "{}".to_string(),
                            thought_signature: None,
                        },
                    ],
                },
                IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id: "call_a".to_string(),
                        content: r#"{"passed": true}"#.to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id: "call_b".to_string(),
                        content: "all green".to_string(),
                    }],
                },
            ],
            ..IrRequest::default()
        };

        #[expect(
            clippy::expect_used,
            reason = "a tool-call history is constructed to render"
        )]
        let request = render_generate_content_request(&ir).expect("a tool-call history renders");

        let rendered = request_json(&request);

        #[expect(
            clippy::indexing_slicing,
            reason = "the rendered request is an object carrying `contents`"
        )]
        let contents = &rendered["contents"];

        assert_eq!(
            contents,
            &serde_json::json!([
                {"role": "user", "parts": [{"text": "Run both checks"}]},
                {"role": "model", "parts": [
                    {"functionCall": {"id": "call_a", "name": "lint", "args": {}}},
                    {"functionCall": {"id": "call_b", "name": "test", "args": {}}},
                ]},
                {"role": "user", "parts": [
                    {"functionResponse": {
                        "id": "call_a", "name": "lint", "response": {"passed": true},
                    }},
                    {"functionResponse": {
                        "id": "call_b", "name": "test", "response": {"result": "all green"},
                    }},
                ]},
            ]),
        );
    }

    /// A populated upstream `functionCall.id` is the cross-turn matching
    /// channel: the parse must seed the IR call id with it — on both the
    /// delta and the done event, since the delta is what reveals the id to
    /// the client — and the replayed history must carry it on the
    /// `functionCall` part and the matching `functionResponse` part alike.
    ///
    /// # Panics
    ///
    /// Panics if the parsed events or rendered history drop the upstream id.
    #[test]
    fn upstream_call_id_seeds_the_ir_and_replays_on_call_and_response() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{
                    "functionCall": {"id": "fc-777", "name": "weather", "args": {"city": "Oslo"}},
                }]},
                "finishReason": "STOP",
            }],
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the id-bearing call is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body)
            .expect("an id-bearing function call parses");

        let delta_id = events.iter().find_map(|event| match event {
            IrEvent::ToolCallDelta { call_id, .. } => call_id.clone(),
            _ => None,
        });

        let done = events.iter().find_map(|event| match event {
            IrEvent::ToolCallDone { call_id, .. } => Some(call_id.clone()),
            _ => None,
        });

        assert_eq!(delta_id.as_deref(), Some("fc-777"));
        assert_eq!(done.as_deref(), Some("fc-777"));

        #[expect(
            clippy::expect_used,
            reason = "the preceding assertion pins the done event's id"
        )]
        let call_id = done.expect("the done event carries the upstream call id");

        let ir = IrRequest {
            messages: vec![
                IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![IrPart::ToolCall {
                        call_id: call_id.clone(),
                        name: "weather".to_string(),
                        arguments: r#"{"city":"Oslo"}"#.to_string(),
                        thought_signature: None,
                    }],
                },
                IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id,
                        content: r#"{"temp": 4}"#.to_string(),
                    }],
                },
            ],
            ..IrRequest::default()
        };

        #[expect(
            clippy::expect_used,
            reason = "a tool history is constructed to render"
        )]
        let request = render_generate_content_request(&ir).expect("a tool history renders");

        let rendered = request_json(&request);

        #[expect(
            clippy::indexing_slicing,
            reason = "the rendered request is an object carrying `contents`"
        )]
        let contents = &rendered["contents"];

        assert_eq!(
            contents,
            &serde_json::json!([
                {"role": "model", "parts": [{
                    "functionCall": {"id": "fc-777", "name": "weather", "args": {"city": "Oslo"}},
                }]},
                {"role": "user", "parts": [{
                    "functionResponse": {
                        "id": "fc-777",
                        "name": "weather",
                        "response": {"temp": 4},
                    },
                }]},
            ]),
        );
    }

    /// An id-less upstream call mints one id — identical on the delta and
    /// the done event — and the replayed history pairs the call and its
    /// response under that same minted id, so the matching channel exists
    /// even when the upstream declined to name the call.
    ///
    /// # Panics
    ///
    /// Panics if the minted id fails to pair the call and its response.
    #[test]
    fn id_less_call_mints_one_id_shared_by_call_and_response() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{
                    "functionCall": {"name": "weather", "args": {}},
                }]},
                "finishReason": "STOP",
            }],
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the id-less call is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body)
            .expect("an id-less function call parses");

        let delta_id = events.iter().find_map(|event| match event {
            IrEvent::ToolCallDelta { call_id, .. } => call_id.clone(),
            _ => None,
        });

        let done = events.iter().find_map(|event| match event {
            IrEvent::ToolCallDone {
                call_id,
                name,
                arguments,
                ..
            } => Some((call_id.clone(), name.clone(), arguments.clone())),
            _ => None,
        });

        #[expect(
            clippy::expect_used,
            reason = "the parsed stream is constructed to carry a done event"
        )]
        let (call_id, name, arguments) = done.expect("the done event carries the minted call id");

        assert_eq!(delta_id.as_deref(), Some(call_id.as_str()));
        assert_eq!(name, "weather");
        assert_eq!(arguments, "{}");

        let ir = IrRequest {
            messages: vec![
                IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![IrPart::ToolCall {
                        call_id: call_id.clone(),
                        name,
                        arguments,
                        thought_signature: None,
                    }],
                },
                IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id,
                        content: r#"{"temp": 21}"#.to_string(),
                    }],
                },
            ],
            ..IrRequest::default()
        };

        #[expect(
            clippy::expect_used,
            reason = "a tool history is constructed to render"
        )]
        let request = render_generate_content_request(&ir).expect("a tool history renders");

        let request = request_json(&request);

        #[expect(
            clippy::indexing_slicing,
            reason = "the rendered history pins one call part and one response part"
        )]
        let (call, response) = (
            &request["contents"][0]["parts"][0]["functionCall"],
            &request["contents"][1]["parts"][0]["functionResponse"],
        );

        assert_eq!(call["id"], response["id"]);

        #[expect(
            clippy::expect_used,
            reason = "the preceding assertions pin the delta's id"
        )]
        let revealed = delta_id.expect("the delta revealed the minted id");

        assert_eq!(call["id"], revealed);
    }

    /// Two same-name calls with distinct upstream ids replay with distinct
    /// response ids paired per call — the exact ambiguity the id field
    /// exists to resolve. The outputs arrive in reverse order to prove the
    /// pairing follows each part's id, not positional correspondence.
    ///
    /// # Panics
    ///
    /// Panics if the rendered ids diverge from the pinned pairing.
    #[test]
    fn same_name_calls_pair_distinct_response_ids() {
        let ir = IrRequest {
            messages: vec![
                IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![
                        IrPart::ToolCall {
                            call_id: "up-1".to_string(),
                            name: "lookup".to_string(),
                            arguments: r#"{"key":"a"}"#.to_string(),
                            thought_signature: None,
                        },
                        IrPart::ToolCall {
                            call_id: "up-2".to_string(),
                            name: "lookup".to_string(),
                            arguments: r#"{"key":"b"}"#.to_string(),
                            thought_signature: None,
                        },
                    ],
                },
                IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id: "up-2".to_string(),
                        content: r#"{"found": "b"}"#.to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id: "up-1".to_string(),
                        content: r#"{"found": "a"}"#.to_string(),
                    }],
                },
            ],
            ..IrRequest::default()
        };

        #[expect(
            clippy::expect_used,
            reason = "a tool history is constructed to render"
        )]
        let request = render_generate_content_request(&ir).expect("a tool history renders");

        let rendered = request_json(&request);

        #[expect(
            clippy::indexing_slicing,
            reason = "the rendered request is an object carrying `contents`"
        )]
        let contents = &rendered["contents"];

        assert_eq!(
            contents,
            &serde_json::json!([
                {"role": "model", "parts": [
                    {"functionCall": {"id": "up-1", "name": "lookup", "args": {"key": "a"}}},
                    {"functionCall": {"id": "up-2", "name": "lookup", "args": {"key": "b"}}},
                ]},
                {"role": "user", "parts": [
                    {"functionResponse": {
                        "id": "up-2", "name": "lookup", "response": {"found": "b"},
                    }},
                    {"functionResponse": {
                        "id": "up-1", "name": "lookup", "response": {"found": "a"},
                    }},
                ]},
            ]),
        );
    }

    /// Consecutive user messages — legal and common in OpenAI clients, which
    /// have no alternation requirement — coalesce into one `user` content
    /// whose parts keep the messages' encounter order.
    ///
    /// # Panics
    ///
    /// Panics if the rendered contents diverge from the pinned wire shape.
    #[test]
    fn consecutive_user_messages_coalesce_in_part_order() {
        let ir = IrRequest {
            messages: vec![
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: "first question".to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::User,
                    parts: vec![
                        IrPart::Text {
                            text: "look at this".to_string(),
                        },
                        IrPart::Image {
                            image: IrImage::Url("https://example.com/cat.png".to_string()),
                        },
                    ],
                },
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: "now answer".to_string(),
                    }],
                },
            ],
            ..IrRequest::default()
        };

        #[expect(
            clippy::expect_used,
            reason = "a user-run history is constructed to render"
        )]
        let request = render_generate_content_request(&ir).expect("a user-run history renders");

        let rendered = request_json(&request);

        #[expect(
            clippy::indexing_slicing,
            reason = "the rendered request is an object carrying `contents`"
        )]
        let contents = &rendered["contents"];

        assert_eq!(
            contents,
            &serde_json::json!([{"role": "user", "parts": [
                {"text": "first question"},
                {"text": "look at this"},
                {"fileData": {"fileUri": "https://example.com/cat.png"}},
                {"text": "now answer"},
            ]}]),
        );
    }

    /// An already-alternating history must render byte-identically to the
    /// pre-change output, which mapped every message 1:1 onto one content
    /// entry: the pin below fixes those exact serialized bytes.
    ///
    /// # Panics
    ///
    /// Panics if the rendered bytes diverge from the pinned wire shape.
    #[test]
    fn alternating_history_renders_the_pinned_wire_shape() {
        let ir = IrRequest {
            messages: vec![
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: "hello".to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![IrPart::Text {
                        text: "hi there".to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: "one more question".to_string(),
                    }],
                },
            ],
            ..IrRequest::default()
        };

        #[expect(
            clippy::expect_used,
            reason = "an alternating history is constructed to render"
        )]
        let request = render_generate_content_request(&ir).expect("an alternating history renders");

        // The typed contents serialize their fields in declaration order, so
        // the pin below is the exact upstream byte rendering.
        #[expect(clippy::expect_used, reason = "rendered contents always serialize")]
        let rendered =
            serde_json::to_string(&request.contents).expect("rendered contents always serialize");

        assert_eq!(
            rendered,
            concat!(
                r#"[{"role":"user","parts":[{"text":"hello"}]},"#,
                r#"{"role":"model","parts":[{"text":"hi there"}]},"#,
                r#"{"role":"user","parts":[{"text":"one more question"}]}"#,
                r#"]"#,
            ),
        );
    }
}
