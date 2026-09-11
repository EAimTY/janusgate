//! Gemini GenerateContent protocol codec.
//!
//! Speaks the `gemini_generate_content` upstream protocol, both in the
//! Antigravity Code Assist envelope (§5.2) and as a bare OpenAI-compatible
//! upstream protocol (§5.3).

use super::UpstreamEventParser;
use crate::{
    error::ApiError,
    format::{arguments_as_object, json_insert},
    ids::new_uuid,
    ir::{
        IrEvent, IrFinishReason, IrImage, IrPart, IrRequest, IrResponseFormat, IrRole, IrToolChoice,
    },
    provider::endpoints,
    sse::SseFrame,
};
use serde_json::Value;

/// Renders the upstream request URL and payload for an Antigravity provider.
///
/// `base_url` is the provider's Code Assist endpoint; the provider layer owns
/// endpoint knowledge, so it is threaded in rather than owned by the codec.
///
/// # Errors
///
/// Returns [`ApiError`] when the request cannot be rendered.
pub(crate) fn render_antigravity_request(
    ir: &IrRequest,
    base_url: &str,
    upstream_id: &str,
    project: &str,
    is_streaming: bool,
) -> Result<(String, Value), ApiError> {
    let url = render_request_url(base_url, upstream_id, is_streaming, true);

    let envelope = serde_json::json!({
        "project": project,
        "model": upstream_id,
        "request": render_generate_content_request(ir)?,
        "requestType": "agent",
        "userAgent": endpoints::ANTIGRAVITY_USER_AGENT,
        "requestId": format!("agent-{}", new_uuid()),
    });

    Ok((url, envelope))
}

/// Renders the upstream request URL and payload for an OpenAI-compatible
/// Gemini provider, whose body is the bare GenerateContentRequest.
///
/// # Errors
///
/// Returns [`ApiError`] when the request cannot be rendered.
pub(crate) fn render_compatible_request(
    ir: &IrRequest,
    base_url: &str,
    upstream_id: &str,
    is_streaming: bool,
) -> Result<(String, Value), ApiError> {
    let url = render_request_url(base_url, upstream_id, is_streaming, false);

    Ok((url, render_generate_content_request(ir)?))
}

/// Renders the GenerateContent URL for one call, the single place where the
/// action segment (including its leading colon) is assembled: enveloped Code
/// Assist upstreams take the action directly on the base URL, compatible
/// upstreams insert `/models/<upstream_id>` before it.
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

/// Renders the bare GenerateContentRequest carried in the envelope `request`
/// field or sent directly to a compatible upstream.
///
/// # Errors
///
/// Returns [`ApiError`] for tool call arguments that are not JSON objects and
/// tool outputs whose invocation cannot be resolved.
pub(crate) fn render_generate_content_request(ir: &IrRequest) -> Result<Value, ApiError> {
    let mut contents: Vec<Value> = Vec::new();

    for message in &ir.messages {
        let role = match message.role {
            IrRole::User | IrRole::Tool => "user",
            IrRole::Assistant => "model",
        };

        let parts: Vec<Value> = message
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

        contents.push(serde_json::json!({"role": role, "parts": parts}));
    }

    let mut request = serde_json::json!({"contents": contents});

    if let Some(instructions) = &ir.instructions {
        json_insert(
            &mut request,
            "systemInstruction",
            serde_json::json!({"parts": [{"text": instructions}]}),
        );
    }

    if !ir.tools.is_empty() {
        let declarations: Vec<Value> = ir
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                })
            })
            .collect();

        json_insert(
            &mut request,
            "tools",
            serde_json::json!([{"functionDeclarations": declarations}]),
        );
    }

    if let Some(tool_choice) = &ir.tool_choice {
        let config = match tool_choice {
            IrToolChoice::Auto => serde_json::json!({"mode": "AUTO"}),
            IrToolChoice::None => serde_json::json!({"mode": "NONE"}),
            IrToolChoice::Required => serde_json::json!({"mode": "ANY"}),
            IrToolChoice::Function { name } => {
                serde_json::json!({"mode": "ANY", "allowedFunctionNames": [name]})
            }
        };

        json_insert(
            &mut request,
            "toolConfig",
            serde_json::json!({"functionCallingConfig": config}),
        );
    }

    let mut generation_config = serde_json::Map::new();

    if let Some(temperature) = ir.temperature {
        generation_config.insert("temperature".to_string(), Value::from(temperature));
    }

    if let Some(top_p) = ir.top_p {
        generation_config.insert("topP".to_string(), Value::from(top_p));
    }

    if let Some(max_output_tokens) = ir.max_output_tokens {
        generation_config.insert(
            "maxOutputTokens".to_string(),
            Value::from(max_output_tokens),
        );
    }

    match &ir.response_format {
        Some(IrResponseFormat::Json) => {
            generation_config.insert(
                "responseMimeType".to_string(),
                Value::String("application/json".to_string()),
            );
        }

        Some(IrResponseFormat::JsonSchema { schema, .. }) => {
            generation_config.insert(
                "responseMimeType".to_string(),
                Value::String("application/json".to_string()),
            );

            generation_config.insert("responseSchema".to_string(), schema.clone());
        }

        _ => {}
    }

    if let Some(effort) = ir.reasoning_effort {
        generation_config.insert(
            "thinkingConfig".to_string(),
            serde_json::json!({"thinkingBudget": effort.to_thinking_budget()}),
        );
    }

    if !generation_config.is_empty() {
        json_insert(
            &mut request,
            "generationConfig",
            Value::Object(generation_config),
        );
    }

    Ok(request)
}

/// Resolves the function name a tool output answers, scanning earlier
/// messages for the matching invocation.
///
/// # Errors
///
/// Returns [`ApiError`] when no matching tool call exists in the conversation.
fn resolve_tool_output_name(call_id: &str, ir: &IrRequest) -> Result<String, ApiError> {
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

    Err(ApiError::invalid_parameter(
        "messages",
        format!("Tool output references unknown call_id `{call_id}`"),
    ))
}

/// Renders one IR part into zero or more Gemini parts.
///
/// # Errors
///
/// Returns [`ApiError`] per [`resolve_tool_output_name`] and for non-object
/// tool call arguments.
fn render_part(part: &IrPart, ir: &IrRequest) -> Result<Vec<Value>, ApiError> {
    match part {
        IrPart::Text { text } => Ok(vec![serde_json::json!({"text": text})]),

        IrPart::Image { image } => Ok(vec![match image {
            IrImage::Inline {
                mime_type,
                data_base64,
            } => serde_json::json!({"inlineData": {"mimeType": mime_type, "data": data_base64}}),

            IrImage::Url(url) => serde_json::json!({"fileData": {"fileUri": url}}),
        }]),

        IrPart::ToolCall {
            name,
            arguments,
            thought_signature,
            ..
        } => {
            let mut gemini_part = serde_json::json!({
                "functionCall": {"name": name, "args": arguments_as_object(arguments)?},
            });

            if let Some(signature) = thought_signature {
                json_insert(
                    &mut gemini_part,
                    "thoughtSignature",
                    Value::String(signature.clone()),
                );
            }

            Ok(vec![gemini_part])
        }

        IrPart::ToolOutput { call_id, content } => {
            let name = resolve_tool_output_name(call_id, ir)?;

            let response = match serde_json::from_str::<Value>(content) {
                Ok(Value::Object(map)) => Value::Object(map),
                Ok(other) => serde_json::json!({"result": other}),
                Err(_) => serde_json::json!({"result": content}),
            };

            Ok(vec![serde_json::json!({
                "functionResponse": {"name": name, "response": response},
            })])
        }
    }
}

/// Parses upstream Gemini responses — SSE frames or a full JSON body — into
/// IR events.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Count of function calls synthesized in this response.
    call_counter: usize,

    /// Whether the response delivered any `functionCall` part; Gemini ends
    /// tool-call turns with the plain `STOP` reason, so the delivered calls
    /// decide the canonical tool-call finish.
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

    /// Ingests one GenerateContentResponse object.
    fn ingest_response(&mut self, value: Value) -> Result<Vec<IrEvent>, ApiError> {
        if let Some(error) = value.get("error").filter(|error| error.is_object()) {
            // An error object inside a 2xx body is a malformed upstream
            // payload (README §6.7), so the embedded `code` is never mirrored
            // as the outward status; it rides in the message instead.
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("upstream error");

            let detail = match error.get("code") {
                Some(code) => format!("{message} (upstream error code: {code})"),
                None => message.to_string(),
            };

            return Err(ApiError::upstream_unavailable(detail));
        }

        // The Cloud Code Assist API wraps every GenerateContentResponse —
        // `:generateContent` JSON bodies and `alt=sse` frames alike — in a
        // `response` envelope beside `traceId` and `metadata` (README §5.2),
        // while Gemini-compatible upstreams carry the bare response, which
        // has no top-level `response` field of its own. Unwrap when present.
        let value = match value.get("response") {
            Some(inner) if inner.is_object() => inner,
            _ => &value,
        };

        let mut events = Vec::new();

        if !self.is_started {
            self.is_started = true;

            events.push(IrEvent::MessageStart {
                id: value
                    .get("responseId")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("gen-{}", new_uuid()), str::to_string),
                model: value
                    .get("modelVersion")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            });
        }

        if let Some(candidate) = primary_candidate(value) {
            if let Some(parts) = candidate
                .pointer("/content/parts")
                .and_then(Value::as_array)
            {
                for part in parts {
                    self.ingest_part(part, &mut events);
                }
            }

            if let Some(finish_reason) = candidate.get("finishReason").and_then(Value::as_str) {
                self.is_finished = true;

                events.push(IrEvent::Finish(finish_reason_from_gemini(
                    finish_reason,
                    self.is_call_delivered,
                )));
            }
        }

        if let Some(usage) = value.get("usageMetadata").filter(|usage| usage.is_object()) {
            let reasoning_tokens = usage
                .get("thoughtsTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or(0);

            events.push(IrEvent::Usage {
                input_tokens: usage
                    .get("promptTokenCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                output_tokens: usage
                    .get("candidatesTokenCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    + reasoning_tokens,
            });
        }

        Ok(events)
    }

    /// Ingests one content part of the primary candidate.
    fn ingest_part(&mut self, part: &Value, events: &mut Vec<IrEvent>) {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            let is_thought = part
                .get("thought")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            events.push(if is_thought {
                IrEvent::ReasoningDelta(text.to_string())
            } else {
                IrEvent::TextDelta(text.to_string())
            });
        }

        let Some(function_call) = part.get("functionCall") else {
            return;
        };

        self.is_call_delivered = true;

        let name = function_call
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let args = function_call
            .get("args")
            .cloned()
            .unwrap_or(Value::Object(Default::default()));

        let call_id = format!("call_{}", new_uuid());
        let arguments = Value::Object(args.as_object().cloned().unwrap_or_default()).to_string();

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
            thought_signature: part
                .get("thoughtSignature")
                .and_then(Value::as_str)
                .map(str::to_string),
        });

        self.call_counter += 1;
    }
}

impl UpstreamEventParser for UpstreamParser {
    /// Ingests one upstream SSE frame carrying a GenerateContentResponse.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for malformed JSON and upstream error objects.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, ApiError> {
        let value: Value = serde_json::from_str(&frame.data)
            .map_err(|_| ApiError::upstream_unavailable("malformed JSON in upstream SSE frame"))?;

        self.ingest_response(value)
    }

    /// Parses a complete upstream non-streaming JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for upstream error objects and malformed bodies.
    fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, ApiError> {
        self.ingest_response(value)
    }

    fn finish(&mut self) -> Vec<IrEvent> {
        // An upstream may close its stream cleanly after streaming content
        // without ever sending a `finishReason`; the missing finish reason
        // degrades to `Stop` so the streamed client sees the same terminal
        // finish reason as the aggregated rendering of the same stream.
        if !self.is_finished {
            self.is_finished = true;

            return vec![IrEvent::Finish(IrFinishReason::Stop)];
        }

        Vec::new()
    }
}

/// Extracts the primary candidate, preferring `index == 0`; parallel
/// candidates are not supported and the remainder is ignored.
fn primary_candidate(value: &Value) -> Option<&Value> {
    let candidates = value.get("candidates")?.as_array()?;

    candidates
        .iter()
        .find(|candidate| candidate.get("index").and_then(Value::as_u64) == Some(0))
        .or_else(|| candidates.first())
}

/// Maps a Gemini `finishReason` to the canonical reason.
///
/// Gemini ends function-call turns with the plain `STOP` reason — the
/// `finishReason` vocabulary has no tool-call value — so a turn that
/// delivered `functionCall` parts carries the canonical
/// [`IrFinishReason::ToolCall`] finish, matching official Chat Completions
/// semantics for text+tool-call mixed turns too.
fn finish_reason_from_gemini(value: &str, is_call_delivered: bool) -> IrFinishReason {
    match value {
        "MAX_TOKENS" => IrFinishReason::Length,

        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => {
            IrFinishReason::ContentFilter
        }

        _ if is_call_delivered => IrFinishReason::ToolCall,

        _ => IrFinishReason::Stop,
    }
}
