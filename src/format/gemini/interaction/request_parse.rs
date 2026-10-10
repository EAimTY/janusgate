//! Client-request parsing for the Gemini Interactions codec: the inbound `POST /interactions`
//! body deserialized into the IR request, speaking the official wire vocabulary the parent module
//! pins.
//!
//! The endpoint handler lives in `crate::api::gemini_v1beta::interactions`; shared prefix
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
        InvalidRequest, IrImage, IrPart, IrRequest, IrResponseFormat, IrRole, IrTool, IrToolChoice,
    },
    vocabulary::ReasoningLevel,
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
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
    store: Option<serde::de::IgnoredAny>,

    /// Service tier hint; a stateless gateway cannot honor it downstream, so it is accepted and
    /// ignored.
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
    service_tier: Option<serde::de::IgnoredAny>,

    /// Free-form labels; pure metadata the gateway never reads, so accepted and ignored.
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
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

        // The IR helper names its rejections after the OpenAI member `image_url`; this
        // surface's own member is the block's `content`, so the rejection is re-wrapped here
        // (`docs/reference/inference.md`: every rejection names the field).
        return IrImage::from_url(&uri)
            .map_err(|error| InvalidRequest::field("content", error.to_string()));
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
                // leading run of empty blocks carries none (see `format::join_text_parts`).
                let combined = format::join_text_parts(texts);

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
    #[expect(
        dead_code,
        reason = "accepted and ignored per `docs/reference/inference.md`"
    )]
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
        .map(|level| ReasoningLevel::parse(&level, "thinking_level"))
        .transpose()?;

    Ok(ParsedRequest {
        affinity,
        ir: IrRequest {
            has_compaction_trigger: false,
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
