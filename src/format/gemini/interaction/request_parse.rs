//! Client-request parsing for the Gemini Interactions codec: the inbound `POST /interactions`
//! body deserialized into the IR request, speaking the official wire vocabulary the parent module
//! pins.
//!
//! The endpoint handler lives in [`crate::api::gemini_v1beta::interactions`]; shared prefix
//! concerns (authentication, 404/405 bodies, body limit, accounting row id) live beside it.
//!
//! # Surface contract
//!
//! - The model rides the request **body** (`model`): `POST /interactions` has no URL model slot, so
//!   the surface addresses models in the body like the OpenAI surfaces do. The router threads
//!   [`RouteContext`](crate::format::RouteContext) into parsing carrying only the streaming
//!   override: `?alt=sse` forces streaming, otherwise the body's `stream` member decides.
//! - The body is parsed strictly: unknown members are 400s, and the known-but-unsupported official
//!   controls each reject naming their field — `previous_interaction_id`, `background`,
//!   `cached_content`, and `webhook_config` (the gateway never chains or stores);
//!   `response_modalities` and `response_mime_type` (the gateway renders text outputs; the latter
//!   is the deprecated spelling of `response_format`); `safety_settings`, `agent`, `agent_config`,
//!   and `environment` (model interactions only); and inside `generation_config`: `seed`,
//!   `stop_sequences`, and the `speech_config`/`image_config`/
//!   `video_config`/`transcription_config` modality controls.
//! - Presentation-only controls are accepted and ignored: `store` (in whatever shape it arrives),
//!   `service_tier`, `labels`, and `generation_config.thinking_summaries`.
//! - The official `generation_config` carries the reasoning and tool-choice controls the surface
//!   reads (`max_output_tokens`, `thinking_level`, `tool_choice`) and no sampling members at all —
//!   there is no `temperature` or `top_p` on this surface.
//! - `input` accepts every official union member: a plain string, a single content block, a
//!   content-block array, or the Step array; the non-step forms normalize to one `user_input` step.
//!   Reasoning enters the IR through `thought` steps, whose complete step JSON is stored verbatim
//!   as the block's opaque replay payload; tool calls replay through `function_call` steps carrying
//!   their ids and JSON-object arguments.
//! - `function_result` steps accept a plain string, a text-block array (image blocks are a 400 —
//!   the IR tool output has no media channel), or the tool's own result object, flattened to JSON
//!   text; the official `is_error` member rides into the IR. The optional `name` member is accepted
//!   and ignored: the IR tool output carries no name, and the upstream render resolves it from the
//!   matching call instead.
//! - `response_format` accepts the official `text` member: `text/plain` (or no `mime_type`) is
//!   free-form text; `application/json` is JSON; a `schema` beside it is a named-schema format
//!   under the synthesized name `response` (the official shape has no schema-name slot). The
//!   union's `audio`/`image`/`video` members and a schema without `application/json` are 400s
//!   naming the field.
//! - `tool_choice` accepts the `auto`/`any`/`none` modes as a bare string or as the official object
//!   form `{allowed_tools: {mode?, tools?}}`, whose mode defaults to `any` — the only mode that
//!   takes a tool list; `validated` and multiple allowed tools are 400s, as is a choice without
//!   advertised tools. Built-in/server-side tools are 400s — function tools only.

use super::ResponseFormatShape;
use crate::{
    format::{
        self, ParsedRequest, RouteContext,
        gemini::{GeminiResponseFormatRejection, parse_gemini_response_format},
    },
    ir::{
        InvalidRequest, IrImage, IrPart, IrReasoningLevel, IrRequest, IrResponseFormat, IrRole,
        IrTool, IrToolChoice,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Strictly parsed Interactions request body.
///
/// Unknown members are rejected outright; the known-but-unsupported official controls below are
/// modelled explicitly so their rejections name the offending field instead of rendering serde's
/// generic unknown-field wording.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestBody {
    /// Public model id to route; the Interactions API addresses models in the body.
    model: String,

    /// Conversation input: any official union member, normalized into steps.
    input: InputIn,

    /// System instructions.
    #[serde(default)]
    system_instruction: Option<String>,

    /// Function tools advertised to the model.
    #[serde(default)]
    tools: Option<Vec<ToolIn>>,

    /// Reasoning and tool-choice configuration.
    #[serde(default)]
    generation_config: Option<GenerationConfigIn>,

    /// Response format restriction.
    #[serde(default)]
    response_format: Option<ResponseFormatShape>,

    /// Whether the client wants a streamed response; the `?alt=sse` query overrides it.
    #[serde(default)]
    stream: Option<bool>,

    /// Storage request; the gateway is stateless, so the member is accepted and ignored in
    /// whatever shape it arrives.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    store: Option<serde::de::IgnoredAny>,

    /// Service tier hint; a stateless gateway cannot honor it downstream, so it is accepted and
    /// ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    service_tier: Option<serde::de::IgnoredAny>,

    /// Free-form labels; pure metadata the gateway never reads, so accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    labels: Option<serde::de::IgnoredAny>,

    /// Chained interaction resume; the gateway never chains, so a present member is a 400.
    #[serde(default)]
    previous_interaction_id: Option<serde::de::IgnoredAny>,

    /// Background execution; the gateway never chains, so a present member is a 400.
    #[serde(default)]
    background: Option<serde::de::IgnoredAny>,

    /// Stored-content reference; the gateway is stateless, so a present member is a 400.
    #[serde(default)]
    cached_content: Option<serde::de::IgnoredAny>,

    /// Webhook delivery configuration; pairs with `background`, so a present member is a 400.
    #[serde(default)]
    webhook_config: Option<serde::de::IgnoredAny>,

    /// Output modality configuration; the gateway renders text outputs, so a present member is a
    /// 400.
    #[serde(default)]
    response_modalities: Option<serde::de::IgnoredAny>,

    /// Deprecated output MIME type spelling; `response_format` replaces it, so a present member is
    /// a 400.
    #[serde(default)]
    response_mime_type: Option<serde::de::IgnoredAny>,

    /// Safety thresholds; known-unsupported, so a present member is a 400.
    #[serde(default)]
    safety_settings: Option<serde::de::IgnoredAny>,

    /// Agent-driven interaction; the gateway serves model interactions only, so a present member
    /// is a 400.
    #[serde(default)]
    agent: Option<serde::de::IgnoredAny>,

    /// Agent configuration; the gateway serves model interactions only, so a present member is a
    /// 400.
    #[serde(default)]
    agent_config: Option<serde::de::IgnoredAny>,

    /// Remote execution environment; out of scope, so a present member is a 400.
    #[serde(default)]
    environment: Option<serde::de::IgnoredAny>,
}

/// The official `input` union: a plain string, a single content block, a content-block array, or
/// the Step array. The non-step forms normalize to one `user_input` step.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum InputIn {
    /// The plain-string form: one user turn carrying one text block.
    Text(String),

    /// The single content block: one user turn carrying it.
    Content(ContentBlockIn),

    /// The Step-array form.
    Steps(Vec<StepIn>),

    /// The content-block-array form: one user turn carrying the blocks.
    Contents(Vec<ContentBlockIn>),
}

/// One input step, tagged by the official `type`.
///
/// The shapes are strict: a member outside the step's own shape is rejected. The gateway's own
/// rendered responses replay through these same shapes — `model_output` and `thought` steps it
/// emitted, `function_call` steps carrying the ids and argument objects it minted — so replaying
/// them verbatim parses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum StepIn {
    /// A user turn.
    UserInput {
        /// The turn's content blocks in order.
        content: Vec<ContentBlockIn>,
    },

    /// A model output turn, replayed from an earlier response.
    ModelOutput {
        /// The turn's content blocks in order; non-text blocks ride the IR where it has a slot.
        content: Vec<ContentBlockIn>,
    },

    /// A model reasoning block: its summary blocks plus the signature terminating it. The
    /// complete step JSON becomes the block's opaque replay payload, so the step replays
    /// byte-exact to the Interactions upstream.
    Thought {
        /// Summary content blocks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<Vec<ContentBlockIn>>,

        /// Upstream signature terminating the block.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// A past tool invocation, carrying its id and argument object for cross-turn matching.
    FunctionCall {
        /// Identifier of the invocation, round-tripped to the matching tool output.
        id: String,

        /// Name of the invoked function.
        name: String,

        /// The arguments as the official JSON object; the object's own shape is the tool's schema
        /// to define, so it rides as an opaque JSON object.
        arguments: Map<String, Value>,
    },

    /// A tool result for a prior function call.
    FunctionResult {
        /// Name of the function being answered; accepted and ignored — the upstream render
        /// resolves it from the matching call.
        #[serde(default)]
        name: Option<String>,

        /// Identifier of the invocation being answered.
        call_id: String,

        /// Result content: a plain string, a text-block array, or the tool's own result object
        /// flattened to JSON text.
        result: FunctionResultPayload,

        /// Whether the tool call errored; rides into the IR.
        #[serde(default)]
        is_error: Option<bool>,
    },
}

/// One content block of a step, tagged by the official `type`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ContentBlockIn {
    /// Plain text.
    Text {
        /// The text content.
        text: String,

        /// Citation annotations. The union's shape is the official annotation vocabulary's to
        /// define, so it rides as an opaque JSON value — byte-exact through a thought step's
        /// opaque replay payload, ignored everywhere else (the IR has no annotation channel).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotations: Option<Value>,
    },

    /// Image content in the official spelling: a remote `uri`, or inline base64 `data` beside its
    /// `mime_type`. The members are optional in the shape so the conversion — not serde's shape
    /// check — owns the rejection: user turns and model outputs accept both spellings, tool
    /// results reject images outright, and every rejection names its field.
    Image {
        /// Remote or data URL of the image.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uri: Option<String>,

        /// Base64-encoded image data.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,

        /// MIME type of the inline data; required beside `data`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,

        /// Media resolution hint from the official closed vocabulary; rides a thought step's
        /// opaque byte-exact and is ignored everywhere else.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resolution: Option<String>,
    },
}

impl ContentBlockIn {
    /// The block's text, when it is a text block; image and other blocks carry none.
    fn text(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } => Some(text),

            Self::Image { .. } => None,
        }
    }
}

/// Converts an image block's members into an IR image, requiring exactly one official spelling.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `content` for a block carrying neither spelling, inline data
/// without its MIME type, both spellings at once, or a URL of an unsupported scheme.
fn image_into_ir(
    uri: Option<String>,
    data: Option<String>,
    mime_type: Option<String>,
) -> Result<IrImage, InvalidRequest> {
    if let Some(uri) = uri {
        if data.is_some() {
            return Err(InvalidRequest::field(
                "content",
                "image blocks carry either `uri` or inline `data`, not both",
            ));
        }

        return IrImage::from_url(&uri);
    }

    match (data, mime_type) {
        (Some(data), Some(mime_type)) => Ok(IrImage::Inline {
            mime_type,
            data_base64: data,
        }),

        (Some(_), None) => Err(InvalidRequest::field(
            "content",
            "inline image blocks require a `mime_type` beside `data`",
        )),

        (None, _) => Err(InvalidRequest::field(
            "content",
            "image blocks require a `uri` or inline `data`",
        )),
    }
}

/// The three accepted shapes of a `function_result` payload: a plain string, a text-block array,
/// or the tool's own result object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum FunctionResultPayload {
    /// The plain-string form.
    Text(String),

    /// The block-array form.
    Blocks(Vec<ContentBlockIn>),

    /// The tool's own result object form, flattened to JSON text.
    Object(Map<String, Value>),
}

impl FunctionResultPayload {
    /// Flattens any shape into the tool-output text the IR carries: strings ride verbatim, text
    /// blocks join with the blank-line separator the other codecs use, and objects render as
    /// their JSON text. Image blocks are rejected: the IR tool output has no media channel, so
    /// silently dropping one would lose request content.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRequest`] naming `result` for image blocks.
    fn into_text(self) -> Result<String, InvalidRequest> {
        match self {
            Self::Text(text) => Ok(text),

            Self::Blocks(blocks) => {
                let texts = blocks
                    .iter()
                    .map(|block| match block {
                        ContentBlockIn::Text { text, .. } => Ok(text.as_str()),

                        ContentBlockIn::Image { .. } => Err(InvalidRequest::field(
                            "result",
                            "function_result content blocks must be text; \
                             `image` is not supported",
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                // The separator only ever separates text the accumulator already holds, so a
                // leading run of empty blocks carries none: skip it, then join the rest with
                // the blank-line separator.
                let combined = texts
                    .iter()
                    .skip_while(|text| text.is_empty())
                    .copied()
                    .collect::<Vec<&str>>()
                    .join("\n\n");

                Ok(combined)
            }

            // Rendering a JSON object map cannot fail: the value holds finite numbers only.
            Self::Object(map) => Ok(Value::Object(map).to_string()),
        }
    }
}

/// The official `generation_config`: the reasoning and tool-choice controls the surface reads,
/// the accepted-and-ignored summary hint, and the known-unsupported controls that reject naming
/// their field. The official configuration pins no sampling members — there is no `temperature`
/// or `top_p` on this surface.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationConfigIn {
    /// Output token budget.
    #[serde(default)]
    max_output_tokens: Option<u64>,

    /// Reasoning level in the closed shared vocabulary.
    #[serde(default)]
    thinking_level: Option<String>,

    /// Tool selection restriction.
    #[serde(default)]
    tool_choice: Option<ToolChoiceIn>,

    /// Summary verbosity hint; the IR has no summary channel, so it is accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    thinking_summaries: Option<serde::de::IgnoredAny>,

    /// Sampling seed; out of scope, so a present member is a 400.
    #[serde(default)]
    seed: Option<serde::de::IgnoredAny>,

    /// Stop sequences; known-unsupported, so a present member is a 400.
    #[serde(default)]
    stop_sequences: Option<serde::de::IgnoredAny>,

    /// Speech output configuration; out of scope, so a present member is a 400.
    #[serde(default)]
    speech_config: Option<serde::de::IgnoredAny>,

    /// Image output configuration; out of scope, so a present member is a 400.
    #[serde(default)]
    image_config: Option<serde::de::IgnoredAny>,

    /// Video output configuration; out of scope, so a present member is a 400.
    #[serde(default)]
    video_config: Option<serde::de::IgnoredAny>,

    /// Transcription configuration; out of scope, so a present member is a 400.
    #[serde(default)]
    transcription_config: Option<serde::de::IgnoredAny>,
}

/// Advertised function tool: function tools only.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolIn {
    /// Tool kind; only `function` is supported.
    #[serde(rename = "type")]
    tool_type: String,

    /// Function name the model calls.
    name: String,

    /// Human-readable description.
    #[serde(default)]
    description: Option<String>,

    /// JSON Schema of the accepted arguments.
    #[serde(default)]
    parameters: Option<Value>,
}

/// Tool choice: a bare mode name, or the official object form pairing an allowed-tools
/// configuration — whose mode defaults to `any`, the only mode that takes a tool list — with its
/// tools.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ToolChoiceIn {
    /// The bare mode form (`auto`, `any`, `none`, `validated`).
    Mode(String),

    /// The official object form: an allowed-tools configuration.
    Object {
        /// The allowed-tools configuration.
        allowed_tools: AllowedToolsIn,
    },
}

/// The allowed-tools configuration of the official object form.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AllowedToolsIn {
    /// Selection mode; defaults to `any`.
    #[serde(default)]
    mode: Option<String>,

    /// The allowed functions.
    #[serde(default)]
    tools: Option<Vec<String>>,
}

/// Parses an Interactions request body into IR with the router's route context: the URL carries
/// no model on this surface, so the body must name it; only the streaming mode may arrive from
/// the route (`?alt=sse` overriding the body's `stream` member).
///
/// # Errors
///
/// Returns [`InvalidRequest`] for malformed or unsupported requests.
///
/// # Panics
///
/// Panics if re-serializing a parsed thought step fails, which cannot occur for the strictly
/// parsed first-party shape.
pub(super) fn parse_request(
    body: &[u8],
    route: &RouteContext,
) -> Result<ParsedRequest, InvalidRequest> {
    let request: RequestBody = format::parse_client_request(body)?;

    reject_present_fields(&request)?;

    let steps = normalize_input(request.input);

    // Consecutive same-role steps coalesce into one message per run, so a text turn followed by
    // its thought step and tool calls stays one assistant message.
    let mut run = format::RoleRun::default();
    let mut affinity =
        crate::format::affinity::Sources::new(crate::format::UpstreamFormat::GeminiInteraction);

    for step in steps {
        match step {
            StepIn::UserInput { content } => {
                run.push(IrRole::User, content_into_parts(content)?);
            }

            StepIn::ModelOutput { content } => {
                run.push(IrRole::Assistant, content_into_parts(content)?);
            }

            StepIn::Thought { summary, signature } => {
                run.push(IrRole::Assistant, vec![thought_part(summary, signature)]);
            }

            StepIn::FunctionCall {
                id,
                name,
                arguments,
            } => {
                affinity.call(true);
                run.push(
                    IrRole::Assistant,
                    vec![IrPart::ToolCall {
                        call_id: id,
                        name,
                        arguments: Value::Object(arguments).to_string(),
                        thought_signature: None,
                    }],
                );
            }

            StepIn::FunctionResult {
                name: _,
                call_id,
                result,
                is_error,
            } => {
                let shape = match &result {
                    FunctionResultPayload::Object(_) => crate::format::affinity::ResultShape::Json,
                    _ => crate::format::affinity::ResultShape::Text,
                };
                affinity.result(true, shape);
                run.push(
                    IrRole::Tool,
                    vec![IrPart::ToolOutput {
                        call_id,
                        content: result.into_text()?,
                        is_error: is_error.unwrap_or(false),
                    }],
                );
            }
        }
    }

    let messages = run.finish();

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

    let generation_config = request.generation_config.unwrap_or_default();
    reject_unsupported_generation_config(&generation_config)?;

    let tool_choice = match generation_config.tool_choice {
        None => None,

        Some(choice) => {
            format::reject_tool_choice_without_tools(tools.is_empty())?;

            Some(parse_tool_choice(choice)?)
        }
    };

    let response_format = parse_response_format(request.response_format)?;

    let reasoning_effort = generation_config
        .thinking_level
        .map(|level| IrReasoningLevel::parse(&level, "thinking_level"))
        .transpose()?;

    Ok(ParsedRequest {
        affinity,
        ir: IrRequest {
            state_items: Vec::new(),
            // The model rides the body on this surface: `POST /interactions` has no URL model
            // slot, so the route context carries no public id and the body's member decides.
            model: request.model,
            instructions: request.system_instruction,
            messages,
            tools,
            tool_choice,
            // The official generation_config pins no sampling members: this surface has no
            // temperature or top_p to parse.
            temperature: None,
            top_p: None,
            max_output_tokens: generation_config.max_output_tokens,
            response_format,
            reasoning_effort,
        },
        // The `?alt=sse` query forces streaming; otherwise the body's `stream` member decides.
        is_streaming: route
            .is_streaming
            .unwrap_or(request.stream.unwrap_or(false)),
        // Usage is always included on this surface; only Chat Completions exposes the option.
        should_include_usage: false,
        compaction: None,
    })
}

/// Normalizes every official `input` union member into the Step array the walk consumes.
fn normalize_input(input: InputIn) -> Vec<StepIn> {
    match input {
        InputIn::Text(text) => vec![StepIn::UserInput {
            content: vec![ContentBlockIn::Text {
                text,
                annotations: None,
            }],
        }],

        InputIn::Content(block) => vec![StepIn::UserInput {
            content: vec![block],
        }],

        InputIn::Steps(steps) => steps,

        InputIn::Contents(content) => vec![StepIn::UserInput { content }],
    }
}

/// Converts content blocks into IR parts: text blocks into text parts, image blocks into image
/// parts. A malformed image block rejects naming `content`.
///
/// # Errors
///
/// Returns [`InvalidRequest`] per [`image_into_ir`].
fn content_into_parts(content: Vec<ContentBlockIn>) -> Result<Vec<IrPart>, InvalidRequest> {
    let mut parts = Vec::with_capacity(content.len());

    for block in content {
        match block {
            ContentBlockIn::Text { text, .. } => parts.push(IrPart::Text { text }),

            ContentBlockIn::Image {
                uri,
                data,
                mime_type,
                resolution: _,
            } => parts.push(IrPart::Image {
                image: image_into_ir(uri, data, mime_type)?,
            }),
        }
    }

    Ok(parts)
}

/// Rejects every known-but-unsupported `generation_config` control the request carries, each
/// naming its own field.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming the first present control.
fn reject_unsupported_generation_config(config: &GenerationConfigIn) -> Result<(), InvalidRequest> {
    for (present, field, reason) in [
        (config.seed.is_some(), "seed", "seed is not supported"),
        (
            config.stop_sequences.is_some(),
            "stop_sequences",
            "stop_sequences is not supported",
        ),
        (
            config.speech_config.is_some(),
            "speech_config",
            "speech_config is not supported; the gateway renders text outputs",
        ),
        (
            config.image_config.is_some(),
            "image_config",
            "image_config is not supported; the gateway renders text outputs",
        ),
        (
            config.video_config.is_some(),
            "video_config",
            "video_config is not supported; the gateway renders text outputs",
        ),
        (
            config.transcription_config.is_some(),
            "transcription_config",
            "transcription_config is not supported",
        ),
    ] {
        if present {
            return Err(InvalidRequest::field(field, reason));
        }
    }

    Ok(())
}

/// Parses the `response_format` member into IR: the official `text` member only, whose `mime_type`
/// defaults to `text/plain` and otherwise delegates to the shared family table (see
/// [`parse_gemini_response_format`]), and whose optional schema completes the named-schema form
/// under the synthesized name. The union's media members reject here, this surface being the only
/// one that carries them on the wire. Every rejection names this surface's `response_format`
/// member.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `response_format` for the union's media members, an
/// unsupported MIME type, or a schema without `application/json`.
fn parse_response_format(
    response_format: Option<ResponseFormatShape>,
) -> Result<Option<IrResponseFormat>, InvalidRequest> {
    let (mime_type, schema) = match response_format.as_ref() {
        None => return Ok(None),

        Some(ResponseFormatShape::Audio {})
        | Some(ResponseFormatShape::Image {})
        | Some(ResponseFormatShape::Video {}) => {
            return Err(InvalidRequest::field(
                "response_format",
                "audio, image, and video response formats are not supported; \
                 the gateway renders text outputs",
            ));
        }

        Some(ResponseFormatShape::Text { mime_type, schema }) => (
            mime_type.as_deref().unwrap_or("text/plain"),
            schema.as_ref(),
        ),
    };

    parse_gemini_response_format(mime_type, schema).map_err(|rejection| match rejection {
        GeminiResponseFormatRejection::SchemaWithoutJsonMime => InvalidRequest::field(
            "response_format",
            "response_format schema requires mime_type `application/json`",
        ),

        GeminiResponseFormatRejection::UnsupportedMime(mime_type) => InvalidRequest::field(
            "response_format",
            format!("response_format mime_type `{mime_type}` is not supported"),
        ),
    })
}

/// Rejects every known-but-unsupported control the request carries, each naming its own field.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming the first present control.
fn reject_present_fields(request: &RequestBody) -> Result<(), InvalidRequest> {
    // Each control rejects with the wording its disposition row documents: the statelessness
    // controls name the invariant, the media and modality controls name the text-output scope.
    if request.previous_interaction_id.is_some() {
        return Err(InvalidRequest::field(
            "previous_interaction_id",
            "The gateway is stateless; previous_interaction_id is not supported",
        ));
    }

    if request.background.is_some() {
        return Err(InvalidRequest::field(
            "background",
            "The gateway is stateless; background is not supported",
        ));
    }

    if request.cached_content.is_some() {
        return Err(InvalidRequest::field(
            "cached_content",
            "The gateway is stateless; cached_content is not supported",
        ));
    }

    if request.webhook_config.is_some() {
        return Err(InvalidRequest::field(
            "webhook_config",
            "webhook_config is not supported; the gateway never chains",
        ));
    }

    if request.response_modalities.is_some() {
        return Err(InvalidRequest::field(
            "response_modalities",
            "response_modalities is not supported; the gateway renders text outputs",
        ));
    }

    if request.response_mime_type.is_some() {
        return Err(InvalidRequest::field(
            "response_mime_type",
            "response_mime_type is deprecated; use response_format",
        ));
    }

    if request.safety_settings.is_some() {
        return Err(InvalidRequest::field(
            "safety_settings",
            "safety_settings is not supported",
        ));
    }

    if request.agent.is_some() {
        return Err(InvalidRequest::field(
            "agent",
            "agent interactions are not supported; the gateway serves model interactions",
        ));
    }

    if request.agent_config.is_some() {
        return Err(InvalidRequest::field(
            "agent_config",
            "agent_config is not supported; the gateway serves model interactions",
        ));
    }

    if request.environment.is_some() {
        return Err(InvalidRequest::field(
            "environment",
            "environment is not supported",
        ));
    }

    Ok(())
}

/// Parses the `tool_choice` member into IR: the `auto`/`any`/`none` modes, with `any` plus one
/// allowed tool forcing that function; the official object form's mode defaults to `any`.
/// `validated` and multiple allowed tools are 400s.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `tool_choice` for unknown or unsupported choices.
fn parse_tool_choice(choice: ToolChoiceIn) -> Result<IrToolChoice, InvalidRequest> {
    let (mode, tools) = match choice {
        ToolChoiceIn::Mode(mode) => (mode, None),

        ToolChoiceIn::Object { allowed_tools } => (
            allowed_tools.mode.unwrap_or_else(|| "any".to_string()),
            allowed_tools.tools,
        ),
    };

    match mode.as_str() {
        "auto" => {
            reject_allowed_tools(tools.as_deref(), "auto")?;

            Ok(IrToolChoice::Auto)
        }

        "any" => match tools.as_deref() {
            None | Some([]) => Ok(IrToolChoice::Required),

            Some([name]) => Ok(IrToolChoice::Function { name: name.clone() }),

            Some(names) => Err(InvalidRequest::field(
                "tool_choice",
                format!(
                    "tool_choice allows at most one entry in allowed_tools, got {}",
                    names.len(),
                ),
            )),
        },

        "none" => {
            reject_allowed_tools(tools.as_deref(), "none")?;

            Ok(IrToolChoice::None)
        }

        "validated" => Err(InvalidRequest::field(
            "tool_choice",
            "tool_choice `validated` is not supported",
        )),

        other => Err(InvalidRequest::field(
            "tool_choice",
            format!("Unknown tool_choice `{other}`"),
        )),
    }
}

/// Rejects an allowed-tools list paired with a mode that takes none.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `tool_choice` when the list is present and non-empty.
fn reject_allowed_tools(
    allowed_tools: Option<&[String]>,
    mode: &str,
) -> Result<(), InvalidRequest> {
    if allowed_tools.is_some_and(|names| !names.is_empty()) {
        return Err(InvalidRequest::field(
            "tool_choice",
            format!("tool_choice `{mode}` takes no allowed_tools"),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses one request body given as JSON with an empty route context.
    fn parse(body: Value) -> Result<ParsedRequest, InvalidRequest> {
        parse_request(body.to_string().as_bytes(), &RouteContext::empty())
    }

    /// A full request parses into the documented IR: the model from the body, instructions,
    /// coalesced messages carrying text, a thought step with its verbatim opaque, a tool call
    /// with its JSON-object arguments, and the matching tool output with its `is_error`, plus
    /// tools, the generation_config controls, the response format, and the streaming flag.
    ///
    /// # Panics
    ///
    /// Panics if the parsed request diverges from the documented IR.
    #[test]
    fn parses_a_full_request() {
        #[expect(
            clippy::expect_used,
            reason = "the fixture body is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "public-model",
            "system_instruction": "be terse",
            "input": [
                {"type": "user_input", "content": [{"type": "text", "text": "hi"}]},
                {"type": "model_output", "content": [{"type": "text", "text": "hello"}]},
                {"type": "thought", "summary": [
                    {"type": "text", "text": "considering"},
                ], "signature": "sig-1"},
                {"type": "function_call", "id": "call_1", "name": "search", "arguments": {
                    "q": "hi",
                }},
                {"type": "function_result", "name": "search", "call_id": "call_1", "result": [
                    {"type": "text", "text": "found"},
                ], "is_error": false},
            ],
            "tools": [{
                "type": "function",
                "name": "search",
                "description": "Searches.",
                "parameters": {"type": "object"},
            }],
            "generation_config": {
                "max_output_tokens": 100,
                "thinking_level": "high",
                "tool_choice": {"allowed_tools": {"mode": "any", "tools": ["search"]}},
            },
            "response_format": {"type": "text", "mime_type": "application/json"},
            "stream": true,
        }))
        .expect("the fixture body must parse");

        assert_eq!(parsed.ir.model, "public-model");
        assert_eq!(parsed.ir.instructions.as_deref(), Some("be terse"));
        assert!(parsed.is_streaming);

        assert_eq!(parsed.ir.reasoning_effort, Some(IrReasoningLevel::High),);
        assert_eq!(parsed.ir.response_format, Some(IrResponseFormat::Json),);
        assert_eq!(
            parsed.ir.tool_choice,
            Some(IrToolChoice::Function {
                name: "search".to_string(),
            }),
        );
        assert_eq!(parsed.ir.tools.len(), 1);
        assert_eq!(parsed.ir.max_output_tokens, Some(100));
        // The official generation_config pins no sampling members.
        assert_eq!(parsed.ir.temperature, None);
        assert_eq!(parsed.ir.top_p, None);

        assert_eq!(parsed.ir.messages.len(), 3);

        #[expect(
            clippy::indexing_slicing,
            reason = "the preceding length assertion guarantees three messages"
        )]
        let assistant = &parsed.ir.messages[1];

        assert!(assistant.parts.iter().any(|part| matches!(
            part,
            IrPart::Text { text, .. } if text == "hello"
        )));

        let thought = assistant.parts.iter().find_map(|part| match part {
            IrPart::Reasoning {
                text,
                signature,
                opaque,
            } => Some((text, signature, opaque)),

            _ => None,
        });

        #[expect(clippy::expect_used, reason = "the fixture carries the thought step")]
        let thought = thought.expect("the assistant message carries the thought step");

        assert_eq!(thought.0, "considering");
        assert_eq!(thought.1, &Some("sig-1".to_string()));
        assert_eq!(
            thought.2,
            &serde_json::json!({
                "type": "thought",
                "summary": [{"type": "text", "text": "considering"}],
                "signature": "sig-1",
            }),
        );

        #[expect(
            clippy::indexing_slicing,
            reason = "the preceding length assertion guarantees three messages"
        )]
        #[expect(clippy::expect_used, reason = "the fixture message carries one part")]
        let output = parsed.ir.messages[2]
            .parts
            .first()
            .expect("the tool message carries one part");

        assert!(
            matches!(
                output,
                IrPart::ToolOutput {
                    is_error: false,
                    ..
                }
            ),
            "the official is_error member rides into the IR",
        );
    }

    /// Verifies a `text` response format without a `mime_type` parses as free-form text: the
    /// absent member defaults to `text/plain` before the shared family table decides.
    ///
    /// # Panics
    ///
    /// Panics if the parsed format diverges from the contract.
    #[test]
    fn defaults_the_text_response_mime_type_to_plain_text() {
        #[expect(
            clippy::expect_used,
            reason = "the text format body is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "input": [],
            "response_format": {"type": "text"},
        }))
        .expect("a text response format must parse");

        assert_eq!(parsed.ir.response_format, Some(IrResponseFormat::Text));
    }

    /// Every official `input` union member parses: the plain string, the single content block,
    /// and the content-block array all normalize to one `user_input` step, while the Step array
    /// keeps its steps.
    ///
    /// # Panics
    ///
    /// Panics if a union member diverges from the documented normalization.
    #[test]
    fn normalizes_every_input_union_member() {
        for (input, expected_parts) in [
            (
                serde_json::json!("hi"),
                vec![IrPart::Text {
                    text: "hi".to_string(),
                }],
            ),
            (
                serde_json::json!({"type": "text", "text": "hi"}),
                vec![IrPart::Text {
                    text: "hi".to_string(),
                }],
            ),
            (
                serde_json::json!([
                    {"type": "text", "text": "hi"},
                    {"type": "image", "uri": "https://example.com/i.png"},
                ]),
                vec![
                    IrPart::Text {
                        text: "hi".to_string(),
                    },
                    IrPart::Image {
                        image: IrImage::Url("https://example.com/i.png".to_string()),
                    },
                ],
            ),
        ] {
            #[expect(
                clippy::expect_used,
                reason = "each union member is constructed to parse"
            )]
            let parsed = parse(serde_json::json!({"model": "m", "input": input}))
                .expect("the input member must parse");

            assert_eq!(parsed.ir.messages.len(), 1);

            #[expect(clippy::expect_used, reason = "the fixture parses to one message")]
            let message = parsed.ir.messages.first().expect("one message");

            assert_eq!(message.role, IrRole::User);
            assert_eq!(message.parts, expected_parts);
        }
    }

    /// Consecutive same-role steps coalesce into one message while role changes split them: two
    /// user turns merge, and the tool outputs of one turn ride one tool message.
    ///
    /// # Panics
    ///
    /// Panics if the coalescing diverges from the documented rule.
    #[test]
    fn coalesces_consecutive_same_role_steps() {
        #[expect(
            clippy::expect_used,
            reason = "the fixture body is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "user_input", "content": [{"type": "text", "text": "a"}]},
                {"type": "user_input", "content": [{"type": "text", "text": "b"}]},
                {"type": "function_result", "name": "f", "call_id": "c1", "result": "one"},
                {"type": "function_result", "name": "f", "call_id": "c2", "result": "two"},
            ],
        }))
        .expect("the fixture body must parse");

        assert_eq!(parsed.ir.messages.len(), 2);
        #[expect(
            clippy::indexing_slicing,
            reason = "the preceding length assertion guarantees two messages"
        )]
        {
            assert_eq!(parsed.ir.messages[0].role, IrRole::User);
            assert_eq!(parsed.ir.messages[0].parts.len(), 2);
            assert_eq!(parsed.ir.messages[1].role, IrRole::Tool);
            assert_eq!(parsed.ir.messages[1].parts.len(), 2);
        }
    }

    /// Every disposition-table 400 rejects naming its field: the statelessness controls, the
    /// media and modality controls, the generation_config rejections, the `validated` tool
    /// choice, multiple allowed tools, a choice without tools, built-in tool types, images in
    /// tool results, and unknown members — while `store`, `service_tier`, `labels`, and
    /// `generation_config.thinking_summaries` stay accepted and ignored.
    ///
    /// # Panics
    ///
    /// Panics if a rejection lands outside its documented field or wording.
    #[test]
    fn rejects_known_unsupported_controls_naming_the_field() {
        let base = || {
            serde_json::json!({
                "model": "m",
                "input": [{"type": "user_input", "content": [{"type": "text", "text": "hi"}]}],
            })
        };

        for (field, value) in [
            ("previous_interaction_id", serde_json::json!("int_1")),
            ("background", serde_json::json!(true)),
            ("cached_content", serde_json::json!("c_1")),
            ("webhook_config", serde_json::json!({"uris": []})),
            ("response_modalities", serde_json::json!(["TEXT"])),
            ("response_mime_type", serde_json::json!("application/json")),
            ("safety_settings", serde_json::json!([])),
            ("agent", serde_json::json!("deep-research")),
            ("agent_config", serde_json::json!({"type": "dynamic"})),
            ("environment", serde_json::json!("env_1")),
        ] {
            let mut body = base();

            #[expect(
                clippy::indexing_slicing,
                reason = "the fixture object carries every rejected member slot"
            )]
            {
                body[field] = value;
            }

            #[expect(
                clippy::expect_used,
                reason = "each control is constructed to be rejected"
            )]
            let error = parse(body).expect_err("a known-unsupported control must be rejected");

            assert_eq!(
                error.param.as_deref(),
                Some(field),
                "the rejection must name `{field}`",
            );
        }

        for (field, value) in [
            ("seed", serde_json::json!(7)),
            ("stop_sequences", serde_json::json!(["x"])),
            ("speech_config", serde_json::json!({})),
            ("image_config", serde_json::json!({})),
            ("video_config", serde_json::json!({})),
            ("transcription_config", serde_json::json!({})),
        ] {
            let mut body = base();

            #[expect(
                clippy::indexing_slicing,
                reason = "the fixture object carries every rejected member slot"
            )]
            {
                body["generation_config"] = serde_json::json!({ field: value });
            }

            #[expect(
                clippy::expect_used,
                reason = "each control is constructed to be rejected"
            )]
            let error = parse(body).expect_err("a known-unsupported control must be rejected");

            assert_eq!(
                error.param.as_deref(),
                Some(field),
                "the rejection must name `{field}`",
            );
        }

        for body in [
            serde_json::json!({
                "model": "m",
                "input": [],
                "generation_config": {"tool_choice": "validated"},
                "tools": [{"type": "function", "name": "f"}],
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "generation_config": {
                    "tool_choice": {"allowed_tools": {"mode": "any", "tools": ["a", "b"]}},
                },
                "tools": [{"type": "function", "name": "a"}, {"type": "function", "name": "b"}],
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "generation_config": {"tool_choice": "auto"},
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "tools": [{"type": "google_search", "name": "search"}],
            }),
            serde_json::json!({
                "model": "m",
                "input": [
                    {"type": "function_result", "name": "f", "call_id": "c",
                     "result": [{"type": "image", "uri": "https://example.com/i.png"}]},
                ],
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "generation_config": {"thinking_level": "ultra"},
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "unknown_member": true,
            }),
            // The response_format media members and a schema without application/json reject
            // naming the field.
            serde_json::json!({
                "model": "m",
                "input": [],
                "response_format": {"type": "image"},
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "response_format": {"type": "text", "schema": {"type": "object"}},
            }),
            // The old self-pinned dialect spellings are unknown members today.
            serde_json::json!({
                "model": "m",
                "input": [],
                "temperature": 0.5,
            }),
        ] {
            #[expect(
                clippy::expect_used,
                reason = "each body is constructed to be rejected"
            )]
            let error = parse(body).expect_err("an unsupported request must be rejected");

            assert!(
                error.param.is_some() || !error.to_string().is_empty(),
                "the rejection must carry its cause: {error}",
            );
        }

        // The presentation-only controls parse: `store`, `service_tier`, `labels`, and
        // `generation_config.thinking_summaries` never affect the IR.
        #[expect(
            clippy::expect_used,
            reason = "the ignored-controls body is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "input": [{"type": "user_input", "content": [{"type": "text", "text": "hi"}]}],
            "store": true,
            "service_tier": "standard",
            "labels": {"team": "x"},
            "generation_config": {"thinking_summaries": "auto"},
        }))
        .expect("accepted-and-ignored controls must parse");

        assert_eq!(parsed.ir.messages.len(), 1);
    }

    /// The `function_result` shapes agree: a plain string, a text-block array joining with the
    /// blank-line separator, and the tool's own result object flattened to JSON text all carry
    /// the same content class, while the old JSON-encoded-string arguments spelling — a kind
    /// contradiction against the official object member — is rejected.
    ///
    /// # Panics
    ///
    /// Panics if the shapes diverge from the documented flattening.
    #[test]
    fn flattens_tool_result_shapes() {
        for (result, expected) in [
            (serde_json::json!("plain"), "plain".to_string()),
            (
                serde_json::json!([{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]),
                "a\n\nb".to_string(),
            ),
            (
                serde_json::json!({"passed": true}),
                r#"{"passed":true}"#.to_string(),
            ),
        ] {
            #[expect(
                clippy::expect_used,
                reason = "each result shape is constructed to parse"
            )]
            let parsed = parse(serde_json::json!({
                "model": "m",
                "input": [
                    {"type": "function_result", "name": "f", "call_id": "c", "result": result},
                ],
            }))
            .expect("each result shape must parse");

            assert_eq!(parsed.ir.messages.len(), 1);

            #[expect(
                clippy::expect_used,
                reason = "the preceding length assertion guarantees one message with one part"
            )]
            let output = parsed
                .ir
                .messages
                .first()
                .expect("the result parses to one message")
                .parts
                .first()
                .expect("the message carries one part");
            assert!(
                matches!(output, IrPart::ToolOutput { .. }),
                "a function_result step must parse to a tool output",
            );

            if let IrPart::ToolOutput { content, .. } = output {
                assert_eq!(content, &expected);
            }
        }

        #[expect(
            clippy::expect_used,
            reason = "the old dialect spelling is constructed to be rejected"
        )]
        let error = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "function_call", "id": "c", "name": "f",
                 "arguments": r#"{"q":"hi"}"#},
            ],
        }))
        .expect_err("the old JSON-encoded-string arguments must be rejected");

        assert!(
            !error.to_string().is_empty(),
            "the rejection must carry its cause: {error}",
        );
    }

    /// A thought step's complete JSON — annotation- and resolution-carrying blocks included —
    /// re-serializes byte-exact into the block's opaque payload, so the same-family replay is
    /// verbatim; model_output image blocks ride the IR as image parts.
    ///
    /// # Panics
    ///
    /// Panics if the opaque diverges from the parsed step.
    #[test]
    fn carries_the_thought_step_opaque_byte_exact() {
        #[expect(
            clippy::expect_used,
            reason = "the fixture body is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "thought", "summary": [
                    {"type": "text", "text": "why",
                     "annotations": [{"type": "url_citation", "url": "https://x"}]},
                    {"type": "image", "uri": "https://example.com/t.png",
                     "resolution": "high"},
                ], "signature": "sig-9"},
                {"type": "model_output", "content": [
                    {"type": "text", "text": "answer"},
                    {"type": "image", "data": "aGk=", "mime_type": "image/png"},
                ]},
            ],
        }))
        .expect("the fixture body must parse");

        assert_eq!(parsed.ir.messages.len(), 1);

        #[expect(clippy::expect_used, reason = "the fixture parses to one message")]
        let message = parsed.ir.messages.first().expect("one message");

        #[expect(clippy::expect_used, reason = "the fixture carries the thought step")]
        let opaque = message
            .parts
            .iter()
            .find_map(|part| match part {
                IrPart::Reasoning { opaque, .. } => Some(opaque),

                _ => None,
            })
            .expect("the thought step parses to a reasoning part");

        assert_eq!(
            opaque,
            &serde_json::json!({
                "type": "thought",
                "summary": [
                    {"type": "text", "text": "why",
                     "annotations": [{"type": "url_citation", "url": "https://x"}]},
                    {"type": "image", "uri": "https://example.com/t.png",
                     "resolution": "high"},
                ],
                "signature": "sig-9",
            }),
        );

        assert!(
            message.parts.iter().any(|part| matches!(
                part,
                IrPart::Image {
                    image: IrImage::Inline { .. },
                    ..
                }
            )),
            "model_output image blocks ride the IR as image parts",
        );
    }

    /// Streaming follows the body-then-query rule: the body's `stream` member decides unless the
    /// route carries the `?alt=sse` override, which forces streaming either way.
    ///
    /// # Panics
    ///
    /// Panics if the streaming decision diverges from the convention.
    #[test]
    fn decides_streaming_from_body_unless_alt_sse_overrides() {
        let body = serde_json::json!({
            "model": "m",
            "input": [],
            "stream": true,
        });

        #[expect(
            clippy::expect_used,
            reason = "the fixture bodies are constructed to parse"
        )]
        let streaming = parse_request(body.to_string().as_bytes(), &RouteContext::empty())
            .expect("the streaming body must parse");

        assert!(streaming.is_streaming);

        #[expect(
            clippy::expect_used,
            reason = "the fixture bodies are constructed to parse"
        )]
        let forced = parse_request(
            serde_json::json!({"model": "m", "input": []})
                .to_string()
                .as_bytes(),
            &RouteContext {
                public_id: None,
                is_streaming: Some(true),
                ..RouteContext::default()
            },
        )
        .expect("the overridden body must parse");

        assert!(forced.is_streaming);

        #[expect(
            clippy::expect_used,
            reason = "the fixture bodies are constructed to parse"
        )]
        let buffered = parse_request(
            serde_json::json!({"model": "m", "input": []})
                .to_string()
                .as_bytes(),
            &RouteContext::empty(),
        )
        .expect("the buffered body must parse");

        assert!(!buffered.is_streaming);
    }
}

/// Converts an accepted inbound thought into its reasoning and normalized replay payload.
///
/// # Panics
///
/// Panics if a typed accepted thought cannot serialize, which its fields prevent.
fn thought_part(summary: Option<Vec<ContentBlockIn>>, signature: Option<String>) -> IrPart {
    let text = summary
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(ContentBlockIn::text)
        .collect::<Vec<_>>()
        .join("");
    #[expect(
        clippy::expect_used,
        reason = "the typed accepted thought serializes totally"
    )]
    let opaque = serde_json::to_value(StepIn::Thought {
        summary,
        signature: signature.clone(),
    })
    .expect("accepted thought serializes");
    IrPart::Reasoning {
        text,
        signature,
        opaque,
    }
}
