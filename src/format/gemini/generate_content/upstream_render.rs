//! Upstream-request rendering for the Gemini GenerateContent codec: renders an IR request into
//! the bare GenerateContentRequest upstream payload. The URL — including the envelope the
//! Antigravity definition wraps the body in — belongs to the provider definition (README
//! §5.2).

use crate::{
    format,
    ir::{InvalidRequest, IrImage, IrPart, IrRequest, IrResponseFormat, IrRole, IrToolChoice},
};
use serde::Serialize;
use serde_json::{Map, Value};

/// The `responseMimeType` value requesting a JSON response.
const JSON_MIME_TYPE: &str = "application/json";

/// The bare GenerateContentRequest an upstream `:generateContent` action carries (README §5.3).
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

    /// Thinking configuration, omitted when the client sent no effort string.
    #[serde(rename = "thinkingConfig", skip_serializing_if = "Option::is_none")]
    thinking_config: Option<ThinkingConfig<'a>>,
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

/// Thinking configuration: the client's effort string verbatim in the `thinkingLevel` slot.
///
/// The gateway never renders a token budget (`thinkingBudget`), so reasoning control reaches only
/// Gemini 3 generation and later models; a budget is not this gateway's to invent (README §6.4).
#[derive(Debug, Serialize)]
struct ThinkingConfig<'a> {
    /// Client's reasoning effort string, opaque and verbatim (README §6.4).
    #[serde(rename = "thinkingLevel")]
    thinking_level: &'a str,
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
pub(crate) fn render_generate_content_request(
    ir: &IrRequest,
) -> Result<GenerateContentRequest<'_>, InvalidRequest> {
    // The Gemini API requires `contents` roles to alternate, so consecutive same-role messages
    // coalesce into one content entry through the shared role-run accumulator; `IrRole::Tool`
    // rides the `user` side, the side tool results belong to.
    let mut runs: format::RoleRun<&'static str, Part<'_>> = format::RoleRun::default();

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

        runs.push(role, parts);
    }

    let contents: Vec<Content<'_>> = runs
        .into_runs()
        .into_iter()
        .map(|(role, parts)| Content { role, parts })
        .collect();

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
        thinking_config: ir.reasoning_effort.as_ref().map(|level| ThinkingConfig {
            thinking_level: level.as_str(),
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

/// Renders one IR part into zero or more Gemini parts.
///
/// # Errors
///
/// Returns [`InvalidRequest`] per [`format::gemini::resolve_tool_output_name`] and for non-object
/// tool call arguments.
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

        IrPart::ToolOutput {
            call_id, content, ..
        } => {
            let name = format::gemini::resolve_tool_output_name(call_id, ir)?;

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

        // Reasoning blocks carry no Gemini GenerateContent history slot: the text is a response
        // channel, not a replayable request part for this family, and the opaque payload is
        // family-bound — cross-family it is dropped (README §6.4).
        IrPart::Reasoning { .. } => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::{super::upstream_parse::UpstreamParser, *};
    use crate::{
        format::UpstreamEventParser,
        ir::{IrEvent, IrMessage},
    };

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

    fn thinking_level_of(rendered: &Value) -> Option<&str> {
        rendered
            .get("generationConfig")
            .and_then(|config| config.get("thinkingConfig"))
            .and_then(|thinking| thinking.get("thinkingLevel"))
            .and_then(Value::as_str)
    }

    /// Verifies the Gemini thinking slot of the closed-vocabulary contract (README §6.4): every
    /// vocabulary value renders into `thinkingConfig.thinkingLevel` verbatim, an absent effort
    /// renders no thinking configuration at all, and no rendered body ever carries a token budget.
    ///
    /// # Panics
    ///
    /// Panics if a rendered slot diverges from the contract.
    #[test]
    fn renders_thinking_level_verbatim_and_never_a_budget() {
        use crate::ir::IrReasoningLevel;

        for (level, effort) in [
            (IrReasoningLevel::None, "none"),
            (IrReasoningLevel::Minimal, "minimal"),
            (IrReasoningLevel::Low, "low"),
            (IrReasoningLevel::Medium, "medium"),
            (IrReasoningLevel::High, "high"),
            (IrReasoningLevel::XHigh, "xhigh"),
            (IrReasoningLevel::Max, "max"),
        ] {
            let ir = IrRequest {
                model: "m".to_string(),
                reasoning_effort: Some(level),
                ..IrRequest::default()
            };

            #[expect(clippy::expect_used, reason = "the render is under test")]
            let request = render_generate_content_request(&ir)
                .expect("a request carrying an effort string always renders");
            let rendered = request_json(&request);

            assert_eq!(thinking_level_of(&rendered), Some(effort));
            assert!(
                !rendered.to_string().contains("thinkingBudget"),
                "a token budget must never render"
            );
        }

        let ir = IrRequest {
            model: "m".to_string(),
            ..IrRequest::default()
        };

        #[expect(clippy::expect_used, reason = "the render is under test")]
        let request = render_generate_content_request(&ir)
            .expect("a request without an effort string always renders");
        let rendered = request_json(&request);

        assert!(
            rendered
                .get("generationConfig")
                .is_none_or(|config| config.get("thinkingConfig").is_none()),
            "an absent effort renders no thinking configuration"
        );
        assert!(!rendered.to_string().contains("thinkingBudget"));
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
                        is_error: false,
                    }],
                },
                IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id: "call_b".to_string(),
                        content: "all green".to_string(),
                        is_error: false,
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
                        is_error: false,
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
                        is_error: false,
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
                        is_error: false,
                    }],
                },
                IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id: "up-1".to_string(),
                        content: r#"{"found": "a"}"#.to_string(),
                        is_error: false,
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
