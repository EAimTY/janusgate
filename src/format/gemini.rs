//! Gemini GenerateContent protocol codec.
//!
//! Speaks the `gemini_generate_content` upstream protocol, both in the
//! Antigravity Code Assist envelope (§5.2) and as a bare OpenAI-compatible
//! upstream protocol (§5.3).

use crate::{
    error::ApiError,
    format::{arguments_as_object, json_insert},
    ids::new_uuid,
    ir::{FinishReason, IrEvent, IrImage, IrPart, IrRequest, IrResponseFormat, IrRole, ToolChoice},
    sse::SseFrame,
};
use serde_json::Value;

/// Base URL of the Antigravity Code Assist API.
pub(crate) const ANTIGRAVITY_BASE_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal";

/// Renders the upstream request URL and payload for an Antigravity provider.
///
/// # Errors
///
/// Returns [`ApiError`] when the request cannot be rendered.
pub(crate) fn render_antigravity_request(
    ir: &IrRequest,
    upstream_name: &str,
    project: &str,
    streaming: bool,
) -> Result<(String, Value), ApiError> {
    let action = if streaming {
        "streamGenerateContent?alt=sse"
    } else {
        "generateContent"
    };

    let url = format!("{ANTIGRAVITY_BASE_URL}:{action}");

    let envelope = serde_json::json!({
        "project": project,
        "model": upstream_name,
        "request": render_generate_content_request(ir)?,
        "requestType": "agent",
        "userAgent": "antigravity",
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
    streaming: bool,
) -> Result<(String, Value), ApiError> {
    let action = if streaming {
        ":streamGenerateContent?alt=sse"
    } else {
        ":generateContent"
    };

    let base_url = base_url.trim_end_matches('/');

    let url = format!("{base_url}/models/{upstream_name}{action}");

    Ok((url, render_generate_content_request(ir)?))
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
            ToolChoice::Auto => serde_json::json!({"mode": "AUTO"}),
            ToolChoice::None => serde_json::json!({"mode": "NONE"}),
            ToolChoice::Required => serde_json::json!({"mode": "ANY"}),
            ToolChoice::Function { name } => {
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
}

impl UpstreamParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ingests one upstream SSE frame carrying a GenerateContentResponse.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for malformed JSON and upstream error objects.
    pub(crate) fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, ApiError> {
        let value: Value = serde_json::from_str(&frame.data)
            .map_err(|_| ApiError::upstream_unavailable("malformed JSON in upstream SSE frame"))?;

        self.ingest_response(value)
    }

    /// Parses a complete upstream non-streaming JSON response.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for upstream error objects and malformed bodies.
    pub(crate) fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, ApiError> {
        self.ingest_response(value)
    }

    /// Ingests one GenerateContentResponse object.
    fn ingest_response(&mut self, value: Value) -> Result<Vec<IrEvent>, ApiError> {
        if let Some(error) = value.get("error").filter(|error| error.is_object()) {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("upstream error");

            let status = error
                .get("code")
                .and_then(Value::as_u64)
                .map(|code| code.min(u64::from(u16::MAX)) as u16)
                .unwrap_or(crate::error::UPSTREAM_UNAVAILABLE_STATUS);

            return Err(ApiError::upstream(status, message));
        }

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
                events.push(IrEvent::Finish(finish_reason_from_gemini(finish_reason)));
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
fn finish_reason_from_gemini(value: &str) -> FinishReason {
    match value {
        "MAX_TOKENS" => FinishReason::Length,

        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => {
            FinishReason::ContentFilter
        }

        _ => FinishReason::Stop,
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "tests fail by panicking")]
#[expect(clippy::indexing_slicing, reason = "tests assert exact fixture shapes")]
mod tests {
    use super::{UpstreamParser, render_antigravity_request, render_generate_content_request};
    use crate::{
        ir::{
            FinishReason, IrEvent, IrImage, IrMessage, IrPart, IrRequest, IrRole, ReasoningEffort,
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
            reasoning_effort: Some(ReasoningEffort::Medium),
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
        let frames = sse.ingest(raw.as_bytes()).expect("frames should parse");

        let mut events = Vec::new();

        for frame in frames {
            events.append(&mut parser.ingest_frame(&frame).expect("frame should ingest"));
        }

        events
    }

    /// Reads a nested string value from a JSON fixture.
    ///
    /// # Panics
    ///
    /// Panics when the path is missing; tests assert on exact fixture shapes.
    fn json_str<'a>(value: &'a Value, path: &[&str]) -> &'a str {
        #[expect(clippy::expect_used, reason = "tests assert exact fixture shapes")]
        json_at(value, path)
            .as_str()
            .expect("path exists and is a string")
    }

    /// Reads a nested unsigned number from a JSON fixture.
    ///
    /// # Panics
    ///
    /// Panics when the path is missing; tests assert on exact fixture shapes.
    fn json_u64(value: &Value, path: &[&str]) -> u64 {
        #[expect(clippy::expect_used, reason = "tests assert exact fixture shapes")]
        json_at(value, path)
            .as_u64()
            .expect("path exists and is a number")
    }

    /// Reads a nested JSON value, treating numeric segments as array indices.
    ///
    /// # Panics
    ///
    /// Panics when the path is missing; tests assert on exact fixture shapes.
    fn json_at<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
        let mut current = value;

        for segment in path {
            let next = match segment.parse::<usize>() {
                Ok(index) => current.get(index),
                Err(_) => current.get(*segment),
            };
            let resolved = next.expect("path segment exists in fixture");

            current = resolved;
        }

        current
    }

    /// Verifies the Antigravity envelope contract of §5.2.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn antigravity_envelope_matches_contract() {
        let ir = sample_request();
        let (url, envelope) = render_antigravity_request(&ir, "gemini-2.5-pro", "project-1", true)
            .expect("should render");

        assert_eq!(
            url,
            "https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse"
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
        let done = events
            .iter()
            .find_map(|event| match event {
                IrEvent::ToolCallDone { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .expect("tool call done event");

        assert_eq!(done, format!("call_{}", &done["call_".len()..]));

        assert!(events.contains(&IrEvent::Finish(FinishReason::Stop)));
        assert!(events.contains(&IrEvent::Usage {
            input_tokens: 9,
            output_tokens: 6,
        }));
    }

    /// Verifies that Gemini error objects map status and message.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn upstream_error_objects_map_status_and_message() {
        let mut parser = UpstreamParser::new();
        let error = parser
            .ingest_frame(&crate::sse::SseFrame {
                event: None,
                data: r#"{"error": {"code": 429, "message": "RESOURCE_EXHAUSTED"}}"#.to_string(),
            })
            .expect_err("error object must fail");

        assert_eq!(error.status, 429);
        assert!(error.message.contains("RESOURCE_EXHAUSTED"));
    }
}
