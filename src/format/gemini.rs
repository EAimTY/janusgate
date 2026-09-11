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
    upstream_name: &str,
    project: &str,
    is_streaming: bool,
) -> Result<(String, Value), ApiError> {
    let url = render_request_url(base_url, upstream_name, is_streaming, true);

    let envelope = serde_json::json!({
        "project": project,
        "model": upstream_name,
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
    upstream_name: &str,
    is_streaming: bool,
) -> Result<(String, Value), ApiError> {
    let url = render_request_url(base_url, upstream_name, is_streaming, false);

    Ok((url, render_generate_content_request(ir)?))
}

/// Renders the GenerateContent URL for one call, the single place where the
/// action segment (including its leading colon) is assembled: enveloped Code
/// Assist upstreams take the action directly on the base URL, compatible
/// upstreams insert `/models/<upstream_name>` before it.
fn render_request_url(
    base_url: &str,
    upstream_name: &str,
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
        format!("{base_url}/models/{upstream_name}{action}")
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

        if let Some(candidate) = primary_candidate(&value) {
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

#[cfg(test)]
mod tests {
    use super::{
        UpstreamParser, render_antigravity_request, render_compatible_request,
        render_generate_content_request,
    };
    use crate::{
        format::UpstreamEventParser as _,
        ir::{
            IrEvent, IrFinishReason, IrImage, IrMessage, IrPart, IrReasoningEffort, IrRequest,
            IrRole,
        },
        sse::SseParser,
    };
    use serde_json::Value;

    /// Builds a minimal IR request with one user text turn.
    fn sample_request() -> IrRequest {
        IrRequest {
            model: "gemini-2.5-pro".to_string(),
            instructions: Some("be brief".to_string()),
            messages: vec![IrMessage {
                role: IrRole::User,
                parts: vec![
                    IrPart::Text {
                        text: "look".to_string(),
                    },
                    IrPart::Image {
                        image: IrImage::Inline {
                            mime_type: "image/png".to_string(),
                            data_base64: "QUJD".to_string(),
                        },
                    },
                ],
            }],
            reasoning_effort: Some(IrReasoningEffort::Medium),
            ..IrRequest::default()
        }
    }

    /// Drains raw SSE text through the upstream parser.
    ///
    /// # Panics
    ///
    /// Panics if frame parsing fails.
    fn ingest(parser: &mut UpstreamParser, raw: &str) -> Vec<IrEvent> {
        let mut sse = SseParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the streamed fixture text is well-formed SSE"
        )]
        let frames = sse.ingest(raw.as_bytes()).expect("frames should parse");

        let mut events = Vec::new();

        for frame in frames {
            #[expect(
                clippy::expect_used,
                reason = "every fixture frame is a valid Gemini payload"
            )]
            events.append(&mut parser.ingest_frame(&frame).expect("frame should ingest"));
        }

        events
    }

    /// Reads a nested string value from a JSON fixture.
    ///
    /// # Panics
    ///
    /// Panics when the path is missing; tests query only paths their own
    /// fixtures define.
    fn json_str<'a>(value: &'a Value, path: &[&str]) -> &'a str {
        #[expect(
            clippy::expect_used,
            reason = "tests query only paths their own fixtures define"
        )]
        json_at(value, path)
            .as_str()
            .expect("path exists and is a string")
    }

    /// Reads a nested unsigned number from a JSON fixture.
    ///
    /// # Panics
    ///
    /// Panics when the path is missing; tests query only paths their own
    /// fixtures define.
    fn json_u64(value: &Value, path: &[&str]) -> u64 {
        #[expect(
            clippy::expect_used,
            reason = "tests query only paths their own fixtures define"
        )]
        json_at(value, path)
            .as_u64()
            .expect("path exists and is a number")
    }

    /// Reads a nested JSON value, treating numeric segments as array indices.
    ///
    /// # Panics
    ///
    /// Panics when the path is missing; tests query only paths their own
    /// fixtures define.
    fn json_at<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
        let mut current = value;

        for segment in path {
            let next = match segment.parse::<usize>() {
                Ok(index) => current.get(index),
                Err(_) => current.get(*segment),
            };

            #[expect(
                clippy::expect_used,
                reason = "every path segment is defined by the fixture being asserted"
            )]
            let resolved = next.expect("path segment exists in fixture");

            current = resolved;
        }

        current
    }

    /// Verifies the Antigravity envelope contract of §5.2 and the enveloped
    /// URL assembly.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn antigravity_envelope_matches_contract() {
        let ir = sample_request();

        #[expect(clippy::expect_used, reason = "the sample request always renders")]
        let (url, envelope) = render_antigravity_request(
            &ir,
            "https://codeassist.internal/v1internal",
            "gemini-2.5-pro",
            "project-1",
            true,
        )
        .expect("should render");

        assert_eq!(
            url,
            "https://codeassist.internal/v1internal:streamGenerateContent?alt=sse"
        );

        assert_eq!(json_str(&envelope, &["project"]), "project-1");
        assert_eq!(json_str(&envelope, &["model"]), "gemini-2.5-pro");
        assert_eq!(json_str(&envelope, &["requestType"]), "agent");
        assert_eq!(json_str(&envelope, &["userAgent"]), "antigravity");
        assert!(json_str(&envelope, &["requestId"]).starts_with("agent-"));

        assert_eq!(
            json_str(
                &envelope,
                &["request", "systemInstruction", "parts", "0", "text"]
            ),
            "be brief"
        );
        assert_eq!(
            json_str(&envelope, &["request", "contents", "0", "role"]),
            "user"
        );
        assert_eq!(
            json_str(
                &envelope,
                &["request", "contents", "0", "parts", "0", "text"]
            ),
            "look"
        );
        assert_eq!(
            json_str(
                &envelope,
                &[
                    "request",
                    "contents",
                    "0",
                    "parts",
                    "1",
                    "inlineData",
                    "mimeType"
                ]
            ),
            "image/png"
        );
        assert_eq!(
            json_u64(
                &envelope,
                &[
                    "request",
                    "generationConfig",
                    "thinkingConfig",
                    "thinkingBudget"
                ]
            ),
            8192
        );
    }

    /// Verifies the compatible (bare) URL assembly of §5.3: the model segment
    /// and action are appended after the trimmed base URL.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn compatible_request_url_appends_model_and_action() {
        let ir = sample_request();

        #[expect(clippy::expect_used, reason = "the sample request always renders")]
        let (url, body) =
            render_compatible_request(&ir, "https://upstream.internal/", "gemini-2.5-pro", true)
                .expect("should render");

        assert_eq!(
            url,
            "https://upstream.internal/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );

        assert_eq!(json_str(&body, &["contents", "0", "role"]), "user");
    }

    /// Verifies that tool calls carry signatures and tool outputs resolve
    /// their function names.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn contents_carry_tool_calls_signatures_and_responses() {
        let mut ir = IrRequest {
            model: "gemini".to_string(),
            ..IrRequest::default()
        };

        ir.messages = vec![
            IrMessage {
                role: IrRole::Assistant,
                parts: vec![IrPart::ToolCall {
                    call_id: "call_1".to_string(),
                    name: "weather".to_string(),
                    arguments: r#"{"city":"oslo"}"#.to_string(),
                    thought_signature: Some("sig".to_string()),
                }],
            },
            IrMessage {
                role: IrRole::Tool,
                parts: vec![IrPart::ToolOutput {
                    call_id: "call_1".to_string(),
                    content: r#"{"tempC":5}"#.to_string(),
                }],
            },
        ];
        #[expect(
            clippy::expect_used,
            reason = "a request replaying a known tool call always renders"
        )]
        let request = render_generate_content_request(&ir).expect("should render");

        assert_eq!(json_str(&request, &["contents", "0", "role"]), "model");
        assert_eq!(
            json_str(
                &request,
                &["contents", "0", "parts", "0", "functionCall", "name"]
            ),
            "weather"
        );
        assert_eq!(
            json_str(
                &request,
                &["contents", "0", "parts", "0", "thoughtSignature"]
            ),
            "sig"
        );

        assert_eq!(json_str(&request, &["contents", "1", "role"]), "user");
        assert_eq!(
            json_str(
                &request,
                &["contents", "1", "parts", "0", "functionResponse", "name"]
            ),
            "weather"
        );
        assert_eq!(
            json_u64(
                &request,
                &[
                    "contents",
                    "1",
                    "parts",
                    "0",
                    "functionResponse",
                    "response",
                    "tempC"
                ]
            ),
            5
        );
    }

    /// Verifies that a tool output referencing an unknown call identifier is
    /// rejected.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn tool_output_with_unknown_call_id_is_rejected() {
        let mut ir = IrRequest {
            model: "gemini".to_string(),
            ..IrRequest::default()
        };

        ir.messages = vec![IrMessage {
            role: IrRole::Tool,
            parts: vec![IrPart::ToolOutput {
                call_id: "ghost".to_string(),
                content: "{}".to_string(),
            }],
        }];
        #[expect(clippy::expect_used, reason = "the test asserts this exact rejection")]
        let error =
            render_generate_content_request(&ir).expect_err("unresolvable output must fail");

        assert_eq!(error.status, 400);
    }

    /// Verifies that the Gemini stream parses thought and answer text, tool
    /// calls with signatures, finish reasons, and usage.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_stream_parses_thought_text_calls_finish_and_usage() {
        let mut parser = UpstreamParser::new();

        let events = ingest(
            &mut parser,
            concat!(
                concat!(
                    "data: {\"responseId\":\"r1\",\"modelVersion\":\"gemini-2.5-pro\",",
                    "\"candidates\":[{\"index\":0,\"content\":{\"parts\":[",
                    "{\"text\":\"thinking\",\"thought\":true},",
                    "{\"text\":\"answer\"}]}}]}\n\n"
                ),
                concat!(
                    "data: {\"candidates\":[{\"index\":0,\"content\":{\"parts\":[",
                    "{\"functionCall\":{\"name\":\"weather\",",
                    "\"args\":{\"city\":\"oslo\"}},\"thoughtSignature\":\"sig\"}]},",
                    "\"finishReason\":\"STOP\"}],\"usageMetadata\":",
                    "{\"promptTokenCount\":9,\"candidatesTokenCount\":4,",
                    "\"thoughtsTokenCount\":2}}\n\n"
                ),
            ),
        );
        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture's first frame emits MessageStart first"
        )]
        let first = &events[0];

        assert_eq!(
            first,
            &IrEvent::MessageStart {
                id: "r1".to_string(),
                model: "gemini-2.5-pro".to_string(),
            }
        );

        assert!(events.contains(&IrEvent::ReasoningDelta("thinking".to_string())));
        assert!(events.contains(&IrEvent::TextDelta("answer".to_string())));
        #[expect(
            clippy::expect_used,
            reason = "the fixture carries exactly one tool call"
        )]
        let done = events
            .iter()
            .find_map(|event| match event {
                IrEvent::ToolCallDone { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .expect("tool call done event");

        assert_eq!(done, format!("call_{}", &done["call_".len()..]));

        // The turn delivered a function call, so the plain `STOP` reason maps
        // to the canonical tool-call finish (README §6.3).
        assert!(events.contains(&IrEvent::Finish(IrFinishReason::ToolCall)));
        assert!(events.contains(&IrEvent::Usage {
            input_tokens: 9,
            output_tokens: 6,
        }));
    }

    /// Verifies that Gemini error objects map to a 502 upstream error whose
    /// message keeps the upstream code and text, never mirroring the embedded
    /// code as the outward status (README §6.7).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_error_objects_map_to_502_with_embedded_code() {
        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the test asserts each embedded error object is rejected"
        )]
        let error = parser
            .ingest_frame(&crate::sse::SseFrame {
                event: None,
                data: r#"{"error": {"code": 429, "message": "RESOURCE_EXHAUSTED"}}"#.to_string(),
            })
            .expect_err("error object must fail");

        assert_eq!(error.status, 502);
        assert_eq!(error.code.as_deref(), Some("upstream_error"));
        assert!(error.message.contains("RESOURCE_EXHAUSTED"));
        assert!(error.message.contains("429"));

        // A nonsense 2xx embedded code must not render a success status.
        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the test asserts each embedded error object is rejected"
        )]
        let error = parser
            .ingest_frame(&crate::sse::SseFrame {
                event: None,
                data: r#"{"error": {"code": 200, "message": "odd"}}"#.to_string(),
            })
            .expect_err("error object must fail");

        assert_eq!(error.status, 502);
    }

    /// Degrades a clean end-of-stream that carried no `finishReason` to a
    /// single `Finish(Stop)`, matching the aggregated rendering of the same
    /// stream, whose outward body defaults the finish reason to `stop`; the
    /// streamed client then also sees a terminal finish reason chunk.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn clean_eof_without_finish_reason_yields_finish_stop() {
        let mut parser = UpstreamParser::new();

        let mut events = ingest(
            &mut parser,
            concat!(
                "data: {\"candidates\":[{\"index\":0,\"content\":{\"parts\":[",
                "{\"text\":\"answer\"}]}}]}\n\n"
            ),
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

    /// Finishes a JSON function-call turn with `ToolCall` while a text-only
    /// `STOP` turn keeps finishing `Stop`: Gemini ends function-call turns
    /// with the plain `STOP` reason, so the delivered calls decide the
    /// canonical finish (README §6.3).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_stop_finish_follows_delivered_calls() {
        let mut parser = UpstreamParser::new();

        #[expect(clippy::expect_used, reason = "the fixture is a valid Gemini body")]
        let events = parser
            .parse_json_response(serde_json::json!({
                "candidates": [{"index": 0, "content": {"parts": [
                    {"functionCall": {"name": "weather", "args": {"city": "oslo"}}},
                ]}, "finishReason": "STOP"}],
            }))
            .expect("response should parse");

        assert_eq!(
            events.last(),
            Some(&IrEvent::Finish(IrFinishReason::ToolCall))
        );

        let mut parser = UpstreamParser::new();

        #[expect(clippy::expect_used, reason = "the fixture is a valid Gemini body")]
        let events = parser
            .parse_json_response(serde_json::json!({
                "candidates": [{"index": 0, "content": {"parts": [
                    {"text": "hi"},
                ]}, "finishReason": "STOP"}],
            }))
            .expect("response should parse");

        assert_eq!(events.last(), Some(&IrEvent::Finish(IrFinishReason::Stop)));
    }

    /// Verifies the Code Assist `response` envelope — wrapped around
    /// `:generateContent` JSON bodies and `alt=sse` frames alike — is
    /// unwrapped, with identity and usage read from inside it, while the
    /// bare GenerateContentResponse shape keeps parsing (README §5.2).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_unwraps_code_assist_response_envelope() {
        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the fixture is a valid Code Assist body"
        )]
        let events = parser
            .parse_json_response(serde_json::json!({
                "response": {
                    "candidates": [{
                        "content": {"role": "model", "parts": [{"text": "ok"}]},
                        "finishReason": "STOP",
                    }],
                    "usageMetadata": {
                        "promptTokenCount": 12,
                        "candidatesTokenCount": 4,
                        "totalTokenCount": 16,
                    },
                    "modelVersion": "claude-sonnet-4-6",
                    "responseId": "msg_1",
                },
                "traceId": "trace",
                "metadata": {},
            }))
            .expect("response should parse");

        assert_eq!(
            events.first(),
            Some(&IrEvent::MessageStart {
                id: "msg_1".to_string(),
                model: "claude-sonnet-4-6".to_string(),
            })
        );

        assert!(events.contains(&IrEvent::TextDelta("ok".to_string())));

        assert!(events.contains(&IrEvent::Finish(IrFinishReason::Stop)));

        // Usage is emitted after the finish event by the parser's ordering.
        assert_eq!(
            events.last(),
            Some(&IrEvent::Usage {
                input_tokens: 12,
                output_tokens: 4,
            })
        );

        let mut parser = UpstreamParser::new();

        let mut events = ingest(
            &mut parser,
            concat!(
                "data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",",
                "\"parts\":[{\"text\":\"ok\"}]}}],\"modelVersion\":\"claude-sonnet-4-6\",",
                "\"responseId\":\"req_1\"},\"traceId\":\"trace\",\"metadata\":{}}\n\n"
            ),
        );

        events.append(&mut parser.finish());

        assert_eq!(
            events.first(),
            Some(&IrEvent::MessageStart {
                id: "req_1".to_string(),
                model: "claude-sonnet-4-6".to_string(),
            })
        );

        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, IrEvent::TextDelta(text) if text == "ok"))
                .count(),
            1,
            "the enveloped frame must deliver its text exactly once"
        );

        assert_eq!(events.last(), Some(&IrEvent::Finish(IrFinishReason::Stop)));
    }

    /// Never duplicates the finish reason: a clean end-of-stream after a
    /// chunk that already carried a `finishReason` yields exactly one finish
    /// event.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn clean_eof_after_finish_reason_does_not_duplicate_finish() {
        let mut parser = UpstreamParser::new();

        let mut events = ingest(
            &mut parser,
            concat!(
                "data: {\"candidates\":[{\"index\":0,",
                "\"finishReason\":\"STOP\"}]}\n\n"
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
}
