//! Upstream-request rendering for the Gemini GenerateContent codec: renders an IR request into
//! the bare GenerateContentRequest upstream payload. The URL — including the envelope the
//! Antigravity definition wraps the body in — belongs to the provider definition
//! (`docs/design/providers.md`).

use crate::{
    format,
    ir::{InvalidRequest, IrImage, IrPart, IrRequest, IrResponseFormat, IrRole, IrToolChoice},
};
use serde::Serialize;
use serde_json::{Map, Value};

/// The `responseMimeType` value requesting a JSON response.
const JSON_MIME_TYPE: &str = "application/json";

/// Skip predicate for a borrowed text member that is empty: serde calls the path with the
/// field behind another reference, so the plain `str::is_empty` does not fit as a path.
fn is_empty_str(text: &&str) -> bool {
    text.is_empty()
}

/// The bare GenerateContentRequest an upstream `:generateContent` action carries
/// (`docs/design/providers.md`).
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

    /// A thinking part: replayed model reasoning with the signature the upstream issued for
    /// it. The official `Part` carries the `thought` flag and the `thoughtSignature` beside
    /// `text` as independent members (`ai.google.dev/api/generate-content`), so a
    /// signature-only block renders without a text member.
    Thought {
        /// The reasoning text, omitted when the block carries none.
        #[serde(skip_serializing_if = "is_empty_str")]
        text: &'a str,

        /// Always `true`: the part is model reasoning, never answer text.
        thought: bool,

        /// The replayable thought signature, omitted when the block carries none.
        #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
        thought_signature: Option<&'a str>,
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

    /// Human-readable description, omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,

    /// JSON Schema of the accepted arguments, absent for a function that
    /// takes none.
    #[serde(skip_serializing_if = "Option::is_none")]
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
/// Gemini 3 generation and later models; a budget is not this gateway's to invent
/// (`docs/reference/inference.md`).
#[derive(Debug, Serialize)]
struct ThinkingConfig<'a> {
    /// Client's reasoning effort string, opaque and verbatim (`docs/reference/inference.md`).
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
    // The GenerateContent protocol has no compaction state channel: opaque
    // native state another surface produced is never silently discarded, so
    // its presence fails the render (the caller's replay is producer-bound).
    if !ir.state_items.is_empty() {
        return Err(InvalidRequest::request(
            "the request carries compaction state the Gemini upstream cannot replay",
        ));
    }

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

        // Same-family bridge: a reasoning block the Gemini family issued renders as the
        // thinking part the upstream wire pins — the summary text under `thought: true`,
        // the block's signature riding `thoughtSignature` — because the thinking docs
        // require clients to resend thought blocks exactly as received, and this is the
        // only slot the Interactions replay of a `thought` step can cross into
        // (ai.google.dev/gemini-api/docs/thinking; `docs/design/providers.md`). The opaque payload
        // is family-bound, so family membership is decided by it: a well-formed
        // Interactions thought step (`type: "thought"`) is the Gemini-family credential,
        // while a foreign-family payload — an Anthropic thinking block, an unattributable
        // text-only block whose signature this upstream could not verify — keeps
        // rendering nothing (`docs/reference/inference.md`).
        IrPart::Reasoning {
            text,
            signature,
            opaque,
        } if format::gemini::is_gemini_thought_step(opaque) => Ok(vec![Part::Thought {
            text,
            thought: true,
            thought_signature: signature.as_deref(),
        }]),

        // Reasoning blocks of every other provenance carry no Gemini GenerateContent
        // history slot: the text is a response channel, not a replayable request part for
        // this family, and the opaque payload is family-bound — cross-family it is dropped
        // (`docs/reference/inference.md`).
        IrPart::Reasoning { .. } => Ok(Vec::new()),
    }
}
