//! Client-request parsing for the Responses codec: the outward `POST /responses` body deserialized
//! into the IR request.

use crate::{
    format::{self, ParsedRequest, StringOrArray},
    ir::{
        InvalidRequest, IrImage, IrMessage, IrPart, IrReasoningEffort, IrRequest, IrResponseFormat,
        IrRole, IrTool, IrToolChoice,
    },
};
use serde::{Deserialize, de::IgnoredAny};
use serde_json::Value;
use std::collections::BTreeMap;

/// Strictly parsed Responses request body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestBody {
    /// Public model name to route to.
    model: String,

    /// Conversation input: a plain string or an ordered item array.
    input: StringOrArray<ItemIn>,

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

    // The remaining fields are stateless-idempotent envelope controls that typed clients — the
    // codex CLI serializes every one of them on each request — always carry, but that a stateless
    // gateway can never honor downstream; each is accepted and ignored, like the item-level
    // carve-outs above (README §6.6).
    //
    // `stream_options` has no Responses-specific schema, and `client_metadata` and
    // `access_programs` are client-declared free-form values, so the three are ignored in whatever
    // JSON shape they arrive.
    /// Parallel tool-call switch; the downstream provider decides parallelism, so it is accepted
    /// and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    parallel_tool_calls: Option<bool>,

    /// Extra output content the client asks to have included (codex asks for
    /// `reasoning.encrypted_content`); the gateway renders its own output shapes, so it is
    /// accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    include: Option<Vec<String>>,

    /// Upstream prompt-cache placement key; cache placement is a downstream concern, so it is
    /// accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    prompt_cache_key: Option<String>,

    /// Streaming options; usage is always included, so it is accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    stream_options: IgnoredAny,

    /// Requested service tier; the gateway has no tier control, so it is accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    service_tier: Option<String>,

    /// Free-form client metadata; accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    client_metadata: IgnoredAny,

    /// Client-declared access programs; accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    access_programs: IgnoredAny,
}

/// Input item, tagged by `type`.
///
/// The shapes are strict: a member outside the item's own shape is rejected. The exception is a
/// `reasoning` item, whose payload is accepted and ignored in whatever shape it arrives, because it
/// carries no replayable state — the variant captures every member beside its tag for exactly that
/// reason.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ItemIn {
    /// Conversation message.
    Message {
        /// Gateway-rendered item identifier; accepted and ignored, because codex replays parsed
        /// output items verbatim each turn (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        id: Option<IgnoredAny>,

        /// Authoring role.
        role: String,

        /// Text or typed content.
        content: StringOrArray<PartIn>,

        /// Gateway-rendered item phase; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        phase: Option<IgnoredAny>,

        /// Gateway-rendered item status; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        status: Option<IgnoredAny>,
    },

    /// A past tool invocation.
    FunctionCall {
        /// Gateway-rendered item identifier; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        id: Option<IgnoredAny>,

        /// Invocation identifier.
        call_id: String,

        /// Function name.
        name: String,

        /// JSON-encoded arguments.
        #[serde(default)]
        arguments: String,

        /// Gateway-rendered item status; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        status: Option<IgnoredAny>,

        /// Thought signature to reattach across turns.
        #[serde(default)]
        thought_signature: Option<String>,
    },

    /// A tool result.
    FunctionCallOutput {
        /// Gateway-rendered item identifier; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        id: Option<IgnoredAny>,

        /// Identifier of the invocation being answered.
        call_id: String,

        /// Result content: a plain string or an array of text parts.
        output: StringOrArray<OutputPartIn>,

        /// Gateway-rendered item status; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        status: Option<IgnoredAny>,
    },

    /// Reasoning items carry no replayable state: the payload is accepted and ignored in whatever
    /// shape it arrives, so every member beside the tag lands here.
    Reasoning(BTreeMap<String, IgnoredAny>),
}

impl ItemIn {
    /// Wraps one plain-text user turn: the item a bare-string `input` carries.
    fn user_text(text: String) -> Self {
        Self::Message {
            id: None,
            role: "user".to_string(),
            content: StringOrArray::Text(text),
            phase: None,
            status: None,
        }
    }
}

/// Content part, tagged by `type`; a member outside the part's own shape is rejected.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
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

        /// Gateway-rendered annotations; accepted and ignored, because the gateway's own rendered
        /// `output_text` parts carry the member and replaying an aggregated response verbatim must
        /// parse (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        annotations: Option<IgnoredAny>,

        /// Gateway-rendered log probabilities; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        logprobs: Option<IgnoredAny>,
    },

    /// Image segment.
    InputImage {
        /// Remote or data URL of the image.
        image_url: String,

        /// OpenAI image detail hint; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        detail: Option<IgnoredAny>,
    },
}

/// Content part of a `function_call_output` output array, tagged by `type`.
///
/// Only `input_text` is supported: the IR tool output has no media channel. The image and file
/// kinds are modelled so the rejection can name the offending kind, and every further kind lands in
/// [`OutputPartIn::Other`], which the conversion rejects the same way.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum OutputPartIn {
    /// Text segment.
    InputText {
        /// The text content.
        text: String,
    },

    /// Image segment, which the IR tool output cannot carry.
    InputImage {
        /// Remote or data URL of the image.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        image_url: Option<IgnoredAny>,

        /// OpenAI image detail hint; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        detail: Option<IgnoredAny>,
    },

    /// File segment, which the IR tool output cannot carry.
    InputFile {
        /// Upstream file identifier.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        file_id: Option<IgnoredAny>,

        /// Inline file data.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        file_data: Option<IgnoredAny>,

        /// File name.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        filename: Option<IgnoredAny>,
    },

    /// Any other part kind, of which the tag value the enum machinery consumed leaves no trace.
    #[serde(other)]
    Other,
}

impl OutputPartIn {
    /// The part's wire type name, for the rejection that names the offending kind; [`None`] for a
    /// kind [`OutputPartIn::Other`] swallowed.
    fn type_name(&self) -> Option<&'static str> {
        match self {
            Self::InputText { .. } => Some("input_text"),

            Self::InputImage { .. } => Some("input_image"),

            Self::InputFile { .. } => Some("input_file"),

            Self::Other => None,
        }
    }
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

    /// Output verbosity hint (codex `TextControls`); the IR has no verbosity channel, so it is
    /// accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    verbosity: Option<String>,
}

/// Response format, tagged by `type`; a member outside the format's own shape is rejected, and the
/// fieldless formats are empty struct variants rather than unit variants because only struct
/// variants reject unknown members.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum FormatIn {
    /// Free-form text.
    Text {},

    /// Any valid JSON value.
    JsonObject {},

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

/// Parses a Responses request body into IR.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for malformed JSON, unknown or unsupported fields, invalid parameter
/// ranges, and invalid image references.
pub(super) fn parse_request(body: &[u8]) -> Result<ParsedRequest, InvalidRequest> {
    let request: RequestBody = format::parse_client_request(body)?;

    if request.store == Some(true) {
        return Err(InvalidRequest::field(
            "store",
            "The gateway is stateless; store must be false",
        ));
    }

    if let Some(reasoning) = &request.reasoning
        && let Some(summary) = &reasoning.summary
        && summary != "auto"
    {
        return Err(InvalidRequest::field(
            "summary",
            format!("Unsupported reasoning summary `{summary}`"),
        ));
    }

    format::validate_sampling_ranges(request.temperature, request.top_p)?;

    let mut instruction_parts: Vec<String> = Vec::new();
    let mut messages: Vec<IrMessage> = Vec::new();

    let input_items: Vec<ItemIn> = match request.input {
        StringOrArray::Text(text) => vec![ItemIn::user_text(text)],

        StringOrArray::Array(items) => items,
    };

    for item in input_items {
        match item {
            ItemIn::Message { role, content, .. } => match role.as_str() {
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
                    return Err(InvalidRequest::field(
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
                ..
            } => {
                format::arguments_as_object(&arguments)?;

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

            ItemIn::FunctionCallOutput {
                call_id, output, ..
            } => {
                messages.push(IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id,
                        content: tool_output_text(&output)?,
                    }],
                });
            }

            ItemIn::Reasoning(_payload) => {}
        }
    }

    let instructions = build_instructions(request.instructions, instruction_parts);

    let mut tools: Vec<IrTool> = Vec::new();

    if let Some(tools_in) = &request.tools {
        for tool in tools_in {
            format::reject_non_function_type(&tool.tool_type, "tool")?;

            tools.push(IrTool {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.clone(),
            });
        }
    }

    let tool_choice = match &request.tool_choice {
        None => None,

        Some(choice) => {
            format::reject_tool_choice_without_tools(tools.is_empty())?;

            Some(match choice {
                ToolChoiceIn::Mode(mode) => format::parse_tool_choice_mode(mode)?,

                ToolChoiceIn::Function { tool_type, name } => {
                    format::reject_non_function_type(tool_type, "tool_choice")?;

                    IrToolChoice::Function { name: name.clone() }
                }
            })
        }
    };

    let response_format = match request.text.as_ref().and_then(|text| text.format.as_ref()) {
        None => None,
        Some(FormatIn::Text {}) => Some(IrResponseFormat::Text),
        Some(FormatIn::JsonObject {}) => Some(IrResponseFormat::Json),

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
            InvalidRequest::field("effort", format!("Unknown reasoning effort `{effort}`"))
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

/// Combines the `instructions` field with system/developer message texts in encounter order.
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
/// Returns [`InvalidRequest`] naming the offending content part when the message carries non-text
/// parts: instructions have no image channel, so silently dropping parts would lose request
/// content.
fn message_content_text(content: &StringOrArray<PartIn>) -> Result<String, InvalidRequest> {
    match content {
        StringOrArray::Text(text) => Ok(text.clone()),

        StringOrArray::Array(parts) => {
            let mut combined = String::new();

            for part in parts {
                match part {
                    PartIn::InputText { text } | PartIn::OutputText { text, .. } => {
                        if !combined.is_empty() {
                            combined.push_str("\n\n");
                        }

                        combined.push_str(text);
                    }

                    PartIn::InputImage { .. } => {
                        return Err(InvalidRequest::field(
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
/// Returns [`InvalidRequest`] for malformed image references.
fn message_content_parts(content: &StringOrArray<PartIn>) -> Result<Vec<IrPart>, InvalidRequest> {
    match content {
        StringOrArray::Text(text) => Ok(vec![IrPart::Text { text: text.clone() }]),

        StringOrArray::Array(parts) => {
            let mut converted = Vec::with_capacity(parts.len());

            for part in parts {
                converted.push(match part {
                    PartIn::InputText { text } | PartIn::OutputText { text, .. } => {
                        IrPart::Text { text: text.clone() }
                    }

                    PartIn::InputImage { image_url, .. } => IrPart::Image {
                        image: IrImage::from_url(image_url)?,
                    },
                });
            }

            Ok(converted)
        }
    }
}

/// Extracts plain text from a `function_call_output` item's `output`. Text parts join with the same
/// blank-line separator the Chat Completions codec uses for its text-parts content, so the array
/// form flattens to exactly what the equivalent plain-string form carries.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `output` for a recognized non-text part: the IR tool output
/// has no media channel, so silently dropping such a part would lose request content.
fn tool_output_text(output: &StringOrArray<OutputPartIn>) -> Result<String, InvalidRequest> {
    let parts = match output {
        StringOrArray::Text(text) => return Ok(text.clone()),

        StringOrArray::Array(parts) => parts,
    };

    let mut combined = String::new();

    for part in parts {
        let OutputPartIn::InputText { text } = part else {
            let message = match part.type_name() {
                Some(part_type) => format!(
                    "function_call_output output parts must be text; \
                     `{part_type}` is not supported"
                ),

                None => "function_call_output output parts must be text".to_string(),
            };

            return Err(InvalidRequest::field("output", message));
        };

        if !combined.is_empty() {
            combined.push_str("\n\n");
        }

        combined.push_str(text);
    }

    Ok(combined)
}

#[cfg(test)]
mod tests {
    use super::parse_request;
    use crate::{
        format::ParsedRequest,
        ir::{InvalidRequest, IrPart, IrReasoningEffort, IrResponseFormat, IrRole},
    };
    use serde_json::Value;

    /// Parses one request body given as JSON.
    fn parse(body: Value) -> Result<ParsedRequest, InvalidRequest> {
        parse_request(body.to_string().as_bytes())
    }

    /// A `message` input item carrying the gateway-rendered `id` and `status` fields parses: the
    /// gateway's own assistant message output items carry both, so replaying a response verbatim as
    /// the next request's input — the codex replay loop — must parse (README §6.6 history-replay
    /// carve-outs).
    ///
    /// # Panics
    ///
    /// Panics if the parsed request diverges from the documented IR.
    #[test]
    fn message_input_item_with_status_parses() {
        #[expect(
            clippy::expect_used,
            reason = "the replayed assistant message is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": "hi"},
                {
                    "type": "message",
                    "id": "msg_1",
                    "role": "assistant",
                    "status": "completed",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "hello",
                            "annotations": [],
                            "logprobs": [],
                        },
                    ],
                },
            ],
        }))
        .expect("a replayed assistant message output item must parse");

        assert_eq!(parsed.ir.messages.len(), 2);

        #[expect(
            clippy::indexing_slicing,
            reason = "the preceding length assertion guarantees two messages"
        )]
        let assistant = &parsed.ir.messages[1];

        assert_eq!(assistant.role, IrRole::Assistant);

        assert_eq!(
            assistant.parts,
            vec![IrPart::Text {
                text: "hello".to_string(),
            }],
        );
    }

    /// An `input_image` part carrying the official `detail` hint parses to the same IR as the part
    /// without it: typed SDK clients send `detail` on every `input_image`, and the gateway cannot
    /// honor it downstream, so it is accepted and ignored like `strict` (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed requests diverge from the documented IR.
    #[test]
    fn input_image_detail_is_accepted_and_ignored() {
        #[expect(
            clippy::expect_used,
            reason = "the plain image body is constructed to parse"
        )]
        let plain = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": [
                    {
                        "type": "input_image",
                        "image_url": "https://example.com/i.png",
                    },
                ]},
            ],
        }))
        .expect("a plain input_image part must parse");

        #[expect(
            clippy::expect_used,
            reason = "the detailed image body is constructed to parse"
        )]
        let detailed = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": [
                    {
                        "type": "input_image",
                        "image_url": "https://example.com/i.png",
                        "detail": "high",
                    },
                ]},
            ],
        }))
        .expect("an input_image part carrying `detail` must parse");

        assert_eq!(detailed.ir.messages, plain.ir.messages);
    }

    /// A `function_call_output` item accepts the official plain-string form and the content-parts
    /// array form, and the text-parts array flattens to exactly the string form's tool output with
    /// the blank-line separator (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed requests diverge from the documented IR.
    #[test]
    fn function_call_output_string_and_text_parts_agree() {
        #[expect(
            clippy::expect_used,
            reason = "the string-form output is constructed to parse"
        )]
        let string_form = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "function_call_output", "call_id": "call_1", "output": "x\n\ny"},
            ],
        }))
        .expect("a string output must parse");

        #[expect(
            clippy::expect_used,
            reason = "the parts-form output is constructed to parse"
        )]
        let parts_form = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "function_call_output", "call_id": "call_1", "output": [
                    {"type": "input_text", "text": "x"},
                    {"type": "input_text", "text": "y"},
                ]},
            ],
        }))
        .expect("a text-parts output must parse");

        assert_eq!(parts_form.ir.messages, string_form.ir.messages);

        #[expect(
            clippy::indexing_slicing,
            reason = "the parsed body carries exactly one message"
        )]
        let parts = &parts_form.ir.messages[0].parts;

        assert_eq!(
            *parts,
            vec![IrPart::ToolOutput {
                call_id: "call_1".to_string(),
                content: "x\n\ny".to_string(),
            }],
        );
    }

    /// The full codex CLI request envelope parses: with `wire_api = "responses"` the CLI always
    /// serializes `parallel_tool_calls`, `include`, `prompt_cache_key`, `text.verbosity`,
    /// `service_tier`, `client_metadata`, and `access_programs` beside the supported fields, and
    /// rejecting any of them would 400 the client's first request before the item-level carve-outs
    /// are ever reached (README §6.6). The supported fields inside the envelope — streaming,
    /// reasoning effort, and `text.format` — keep their semantics.
    ///
    /// # Panics
    ///
    /// Panics if the parsed envelope diverges from the documented IR.
    #[test]
    fn codex_envelope_parses() {
        #[expect(
            clippy::expect_used,
            reason = "the codex CLI envelope is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "instructions": "be terse",
            "input": [{"type": "message", "role": "user", "content": "hi"}],
            "tools": [{"type": "function", "name": "shell"}],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "reasoning": {"effort": "high", "summary": "auto"},
            "store": false,
            "stream": true,
            "include": ["reasoning.encrypted_content"],
            "prompt_cache_key": "cache-key",
            "text": {"format": {"type": "text"}, "verbosity": "medium"},
            "service_tier": "auto",
            "client_metadata": {"originator": "codex_cli_rs"},
            "access_programs": ["codex"],
            "stream_options": {"include_usage": true},
        }))
        .expect("the codex CLI request envelope must parse");

        assert!(parsed.is_streaming);

        assert_eq!(parsed.ir.reasoning_effort, Some(IrReasoningEffort::High));

        assert_eq!(parsed.ir.response_format, Some(IrResponseFormat::Text));
    }

    /// The envelope control fields are accepted and ignored: the same request with and without them
    /// parses to identical IR, so their presence never changes routing or the upstream render
    /// (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed requests diverge from the documented IR.
    #[test]
    fn envelope_control_fields_are_accepted_and_ignored() {
        #[expect(
            clippy::expect_used,
            reason = "the plain request is constructed to parse"
        )]
        let plain = parse(serde_json::json!({
            "model": "m",
            "input": "hi",
            "text": {"format": {"type": "json_object"}},
        }))
        .expect("the plain request must parse");

        #[expect(
            clippy::expect_used,
            reason = "the decorated request is constructed to parse"
        )]
        let decorated = parse(serde_json::json!({
            "model": "m",
            "input": "hi",
            "parallel_tool_calls": false,
            "include": ["reasoning.encrypted_content"],
            "prompt_cache_key": "cache-key",
            "stream_options": {"include_usage": true},
            "service_tier": "default",
            "client_metadata": {"originator": "test"},
            "access_programs": ["codex"],
            "text": {"format": {"type": "json_object"}, "verbosity": "low"},
        }))
        .expect("the decorated request must parse");

        assert_eq!(decorated.ir.messages, plain.ir.messages);

        assert_eq!(decorated.ir.response_format, plain.ir.response_format);

        assert_eq!(decorated.is_streaming, plain.is_streaming);
    }

    /// A non-text part in a `function_call_output` output array is rejected naming `output`: the IR
    /// tool output has no media channel, so image and file parts cannot be carried (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if a rejection lands outside its documented wording.
    #[test]
    fn function_call_output_non_text_parts_are_rejected() {
        for part_type in ["input_image", "input_file"] {
            let part = if part_type == "input_image" {
                serde_json::json!({
                    "type": "input_image",
                    "image_url": "https://example.com/i.png",
                })
            } else {
                serde_json::json!({
                    "type": "input_file",
                    "file_id": "file_1",
                })
            };

            #[expect(
                clippy::expect_used,
                reason = "each non-text part is constructed to be rejected"
            )]
            let error = parse(serde_json::json!({
                "model": "m",
                "input": [
                    {"type": "function_call_output", "call_id": "call_1", "output": [
                        {"type": "input_text", "text": "before"},
                        part,
                    ]},
                ],
            }))
            .expect_err("a non-text part in a function_call_output output must be rejected");

            assert_eq!(error.param.as_deref(), Some("output"));

            assert_eq!(
                error.to_string(),
                format!(
                    "function_call_output output parts must be text; \
                     `{part_type}` is not supported"
                ),
            );
        }
    }

    /// Unknown members are rejected wherever the protocol defines a shape: an item member outside
    /// the item's own shape, a content-part member outside the part's own shape, a member beside a
    /// fieldless response format, and a member beside the request envelope's own fields. serde's
    /// rejection names the offending member, which is the contract README §6.6 documents.
    ///
    /// # Panics
    ///
    /// Panics if a rejection lands outside its documented wording.
    #[test]
    fn unknown_members_are_rejected() {
        let cases = [
            (
                serde_json::json!({
                    "model": "m",
                    "input": [
                        {"type": "message", "role": "user", "content": "hi", "bogus": 1},
                    ],
                }),
                "bogus",
            ),
            (
                serde_json::json!({
                    "model": "m",
                    "input": [{
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "hi", "bogus": 1}],
                    }],
                }),
                "bogus",
            ),
            (
                serde_json::json!({
                    "model": "m",
                    "input": "hi",
                    "text": {"format": {"type": "text", "bogus": 1}},
                }),
                "bogus",
            ),
            (
                serde_json::json!({"model": "m", "input": "hi", "bogus": 1}),
                "bogus",
            ),
        ];

        for (body, field) in cases {
            #[expect(
                clippy::expect_used,
                reason = "each unknown member is constructed to be rejected"
            )]
            let error = parse(body).expect_err("an unknown member must be rejected");

            assert!(
                error
                    .to_string()
                    .contains(&format!("unknown field `{field}`")),
                "unexpected rejection: {error}"
            );
        }
    }

    /// A `reasoning` input item carries no replayable state, so its payload is accepted in whatever
    /// shape it arrives — unknown members included — while every other item shape stays strict
    /// (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the reasoning item is rejected or not ignored.
    #[test]
    fn reasoning_items_are_accepted_in_any_shape() {
        #[expect(
            clippy::expect_used,
            reason = "the reasoning payload is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": "hi"},
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [{"type": "summary_text", "text": "thought"}],
                    "encrypted_content": "ciphertext",
                },
            ],
        }))
        .expect("a reasoning item payload must parse in any shape");

        assert_eq!(parsed.ir.messages.len(), 1);
    }

    /// The official `none` effort — the OpenAI vocabulary's explicit no-reasoning value — parses to
    /// the canonical `off` level, the same level `minimal` maps to, and the closed vocabulary still
    /// rejects `xhigh`: the canonical set has no target for it that would not silently downgrade an
    /// explicit client request (README §6.4).
    ///
    /// # Panics
    ///
    /// Panics if the parsed effort or rejection diverges from the contract.
    #[test]
    fn reasoning_effort_none_parses_to_off_and_xhigh_is_rejected() {
        #[expect(clippy::expect_used, reason = "`none` is constructed to parse")]
        let none = parse(serde_json::json!({
            "model": "m",
            "input": "hi",
            "reasoning": {"effort": "none"},
        }))
        .expect("`reasoning.effort: \"none\"` must parse");

        assert_eq!(none.ir.reasoning_effort, Some(IrReasoningEffort::Off));

        #[expect(clippy::expect_used, reason = "`xhigh` is constructed to be rejected")]
        let error = parse(serde_json::json!({
            "model": "m",
            "input": "hi",
            "reasoning": {"effort": "xhigh"},
        }))
        .expect_err("`reasoning.effort: \"xhigh\"` must be rejected");

        assert_eq!(error.param.as_deref(), Some("effort"));

        assert_eq!(error.to_string(), "Unknown reasoning effort `xhigh`");
    }
}
