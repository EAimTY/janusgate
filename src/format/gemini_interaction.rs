//! Gemini Interactions wire codec (`gemini_interaction`).
//!
//! Speaks the `gemini_interaction` upstream protocol — `POST {base}/v1beta/interactions` — and
//! serves the outward `POST /interactions` client surface (see [`inbound`]). Both directions share
//! the one wire vocabulary defined here, so the family is self-consistent by construction.
//!
//! # Wire contract (this module's design choice, per REFACTOR §6)
//!
//! The normative documents pin the semantics but not the JSON member shapes, so this codec fixes
//! them and documents them here:
//!
//! - The model rides the request **body** (`model`), never the URL: `POST /interactions` has no URL
//!   model slot (unlike GenerateContent), so the Interactions API addresses models in the body like
//!   the OpenAI surfaces do. Inbound parsing therefore takes the public model id from the body;
//!   [`crate::format::RouteContext`] carries only the streaming override from the `?alt=sse` query.
//!   Upstream rendering writes the resolved `upstream_id` into the same body member.
//! - Streaming is requested per call: body `stream: true`, or the `?alt=sse` query form — the query
//!   wins when present, mirroring the GenerateContent streaming convention on the single
//!   Interactions endpoint.
//! - Upstream requests always carry `store: false` and never `previous_interaction_id`: the gateway
//!   is stateless and replays the full history through `input` on every call.
//! - `input` is a typed Step array: `user_input` steps (`content` blocks), model `output_text`
//!   steps, model `thought` steps (`id`, `summary`, `signature`), model `function_call` steps
//!   (`id`, `name`, `arguments` as a JSON-encoded string, `signature`), and client
//!   `function_result` steps (`name`, `call_id`, `result` text blocks). Assistant text parts that
//!   have no other slot render as `output_text` steps.
//! - Streaming upstreams speak SSE discriminated on the `event:` name: `interaction.created`,
//!   `step.start`, `step.delta` (delta kinds `text`, `thought_summary`, `thought_signature`,
//!   `tool_arguments`), `step.stop`, and the terminal `interaction.complete` carrying `status` and
//!   `usage`. Non-streaming upstreams answer the Interaction resource (`id`, `model`, `status`,
//!   `steps`, `usage`).
//! - Status maps `requires_action` to tool-call finish, `incomplete` to length, `completed` to
//!   stop, and `failed` to an upstream failure. Usage maps `total_input_tokens` to input,
//!   `total_output_tokens + total_thought_tokens` to output (IR output includes reasoning, as in
//!   the GenerateContent codec), `total_thought_tokens` to reasoning, `total_cached_tokens` to
//!   cached; there is no cache-write accounting and `total_tool_use_tokens` is dropped.
//! - Same-family thought round-trip is verbatim: a reasoning block whose opaque payload is a
//!   well-formed thought step of this family (an object whose `type` is `thought`) replays
//!   byte-for-byte — members and values preserved, key order canonicalized by `serde_json`'s map
//!   ordering — while any other opaque (cross-family blocks, text-only blocks) renders from
//!   `text`/`signature` and drops the payload. The family-membership test is structural, so a
//!   foreign block can never leak onto this family's wire.
//!
//! Upstream payloads are third-party: the response models carry exactly the members the codec
//! reads and ignore every other member, while a member the codec reads whose JSON kind contradicts
//! the contract fails the request as a malformed upstream payload.

pub(crate) mod inbound;

use super::{PayloadError, UpstreamError, UpstreamEventParser};
use crate::{
    format, ids,
    ir::{
        InvalidRequest, IrEvent, IrFinishReason, IrPart, IrRequest, IrResponseFormat, IrRole,
        IrToolChoice, IrUsage,
    },
    sse::SseFrame,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// SSE event opening an upstream stream, carrying the interaction identity.
const EVENT_CREATED: &str = "interaction.created";

/// SSE event opening one step of an upstream stream, carrying the step index and its partial
/// shape.
const EVENT_STEP_START: &str = "step.start";

/// SSE event carrying one incremental delta of an open step.
const EVENT_STEP_DELTA: &str = "step.delta";

/// SSE event closing one step of an upstream stream, carrying the complete step.
const EVENT_STEP_STOP: &str = "step.stop";

/// SSE event terminating an upstream stream, carrying the terminal status and usage.
const EVENT_COMPLETE: &str = "interaction.complete";

/// Terminal status of a completed interaction.
const STATUS_COMPLETED: &str = "completed";

/// Terminal status of an interaction awaiting tool execution.
const STATUS_REQUIRES_ACTION: &str = "requires_action";

/// Terminal status of an interaction cut off before completion.
const STATUS_INCOMPLETE: &str = "incomplete";

/// Terminal status of a failed interaction; surfaces as an upstream failure, never as a finish.
const STATUS_FAILED: &str = "failed";

/// Step type tag of a model text turn.
const STEP_OUTPUT_TEXT: &str = "output_text";

/// Step type tag of a model reasoning block.
const STEP_THOUGHT: &str = "thought";

/// Step type tag of a model tool invocation.
const STEP_FUNCTION_CALL: &str = "function_call";

/// Renders the upstream request URL and payload: `POST {base}/v1beta/interactions` carrying the
/// resolved `upstream_id` as the body `model`, the stateless full-history `input` replay, and
/// always `store: false`.
///
///
/// # Errors
///
/// Returns [`InvalidRequest`] when a tool output answers an unknown call id, or when tool call
/// arguments are not a JSON object.
pub(crate) fn render_request(
    ir: &IrRequest,
    base_url: &str,
    upstream_id: &str,
    is_streaming: bool,
) -> Result<(String, RequestBody), InvalidRequest> {
    let url = format!("{}/v1beta/interactions", base_url.trim_end_matches('/'));

    let body = RequestBody {
        model: upstream_id.to_string(),
        system_instruction: ir.instructions.clone(),
        input: render_input(ir)?,
        tools: render_tools(ir),
        generation_config: render_generation_config(ir),
        response_format: render_response_format(ir),
        stream: is_streaming,
        // The gateway never stores or chains: every call replays the full history statelessly.
        store: false,
    };

    Ok((url, body))
}

/// The upstream Interactions request payload.
///
/// The body owns its strings: synthesized thought steps carry gateway-minted ids, so borrowing the
/// IR alone cannot cover every member.
#[derive(Debug, Serialize)]
pub(crate) struct RequestBody {
    /// Upstream model name, rendered verbatim from the resolved `upstream_id`.
    model: String,

    /// Concatenated system instructions, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<String>,

    /// Stateless full-history replay: every turn so far, oldest first.
    input: Vec<RenderStep>,

    /// Advertised function tools, omitted when the request has none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<UpstreamTool>,

    /// Sampling, reasoning, and tool-choice configuration, omitted when it would be empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,

    /// Response format restriction, omitted for free-form text.
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormatShape>,

    /// Whether the upstream streams its response as SSE.
    stream: bool,

    /// Always `false`: the gateway is stateless.
    store: bool,
}

/// One rendered input step: either a typed step of this family's vocabulary, or a same-family
/// thought step replayed verbatim from its stored value.
///
/// The enum is untagged because the verbatim variant carries a complete step object of its own —
/// serializing it must emit that object as-is, not wrapped in another layer.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum RenderStep {
    /// A same-family thought step replayed verbatim (see the module docs).
    Verbatim(Value),

    /// A step rendered from IR content.
    Typed(TypedStep),
}

/// One typed input step of the Interactions vocabulary.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum TypedStep {
    /// A user turn.
    #[serde(rename = "user_input")]
    UserInput {
        /// The turn's content blocks in order.
        content: Vec<ContentBlock>,
    },

    /// A model text turn.
    #[serde(rename = "output_text")]
    OutputText {
        /// The text content.
        text: String,
    },

    /// A model reasoning block synthesized from IR text and signature.
    #[serde(rename = "thought")]
    Thought {
        /// Gateway-minted step identifier.
        id: String,

        /// Reasoning text, omitted for an empty block.
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,

        /// Upstream signature terminating the block, omitted when absent.
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// A model tool invocation.
    #[serde(rename = "function_call")]
    FunctionCall {
        /// Identifier of the invocation, round-tripped to the matching tool output.
        id: String,

        /// Name of the invoked function.
        name: String,

        /// JSON-encoded arguments object text.
        arguments: String,

        /// Upstream thought signature that must be replayed on the next turn, omitted when
        /// absent.
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// A tool result for a prior function call.
    #[serde(rename = "function_result")]
    FunctionResult {
        /// Name of the function being answered.
        name: String,

        /// Identifier of the invocation being answered.
        call_id: String,

        /// Result content as text blocks.
        result: Vec<ContentBlock>,
    },
}

/// One content block of a `user_input` or `function_result` step.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ContentBlock {
    /// Plain text.
    #[serde(rename = "text")]
    Text {
        /// The text content.
        text: String,
    },

    /// An image reference: an inline data URL or a remote `http`/`https` URL.
    #[serde(rename = "image")]
    Image {
        /// The image URL in [`IrImage::to_url`] form.
        image_url: String,
    },
}

/// One advertised function tool in the typed Interactions shape: function tools only.
#[derive(Debug, Serialize)]
struct UpstreamTool {
    /// Tool kind, always `function`.
    #[serde(rename = "type")]
    tool_type: &'static str,

    /// Function name the model calls.
    name: String,

    /// Human-readable description, omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,

    /// JSON Schema of the accepted arguments, omitted for a function that takes none.
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<Value>,
}

/// Sampling, reasoning, and tool-choice configuration of an upstream request.
#[derive(Debug, Serialize)]
struct GenerationConfig {
    /// Sampling temperature, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,

    /// Nucleus sampling threshold, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,

    /// Output token budget, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u64>,

    /// Reasoning level rendered verbatim from the closed vocabulary, omitted when the client sent
    /// none — then the upstream's own default applies. A token budget is never rendered: reasoning
    /// control reaches only models that accept levels, never a numeric budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_level: Option<&'static str>,

    /// Tool selection restriction, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<UpstreamToolChoice>,
}

impl GenerationConfig {
    /// Whether every field is absent, in which case the whole configuration is omitted from the
    /// request.
    fn is_empty(&self) -> bool {
        self.temperature.is_none()
            && self.top_p.is_none()
            && self.max_output_tokens.is_none()
            && self.thinking_level.is_none()
            && self.tool_choice.is_none()
    }
}

/// Tool selection restriction in the upstream shape: a mode with an optional single allowed
/// function.
#[derive(Debug, Serialize)]
struct UpstreamToolChoice {
    /// Selection mode.
    mode: ToolChoiceMode,

    /// The one function the model may call, present only for a forced single function.
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_tools: Option<Vec<String>>,
}

/// The upstream `tool_choice.mode` values.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ToolChoiceMode {
    /// The model decides freely.
    Auto,

    /// The model must call a tool.
    Any,

    /// The model may not call tools.
    None,
}

/// Response format restriction, shared by the upstream renderer and the inbound parser: the two
/// directions speak the same member shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::format) enum ResponseFormatShape {
    /// Free-form text.
    Text {},

    /// Any valid JSON value.
    Json {},

    /// JSON conforming to a named schema.
    JsonSchema {
        /// Schema name.
        name: String,

        /// The JSON Schema itself, whose shape is the client's to define.
        schema: Value,
    },
}

/// Renders the stateless full-history `input` replay: every message in order, with consecutive
/// user messages coalesced into one `user_input` step whose blocks concatenate — the several
/// turns of plain chat then ride as one step per message, exactly the 1:1 shape.
///
/// A message whose parts all drop renders nothing and is transparent: it contributes no step and
/// cannot split a user run, since it produces no step to alternate against.
///
/// # Errors
///
/// Returns [`InvalidRequest`] per [`resolve_tool_output_name`] and for non-object tool call
/// arguments.
fn render_input(ir: &IrRequest) -> Result<Vec<RenderStep>, InvalidRequest> {
    let mut steps: Vec<RenderStep> = Vec::new();
    let mut user_run: Vec<ContentBlock> = Vec::new();

    // Pushes the open user run as one step; a no-op when the run is empty.
    let flush_user_run = |steps: &mut Vec<RenderStep>, run: &mut Vec<ContentBlock>| {
        if !run.is_empty() {
            steps.push(RenderStep::Typed(TypedStep::UserInput {
                content: std::mem::take(run),
            }));
        }
    };

    for message in &ir.messages {
        match message.role {
            IrRole::User => {
                for part in &message.parts {
                    match part {
                        IrPart::Text { text } => {
                            user_run.push(ContentBlock::Text { text: text.clone() })
                        }

                        IrPart::Image { image } => user_run.push(ContentBlock::Image {
                            image_url: image.to_url(),
                        }),

                        // A user turn has no slot for calls, outputs, or reasoning: the IR type
                        // system never routes them here, and a misplaced part is dropped rather
                        // than rendered into a step whose role would misattribute it.
                        IrPart::ToolCall { .. }
                        | IrPart::ToolOutput { .. }
                        | IrPart::Reasoning { .. } => {}
                    }
                }
            }

            IrRole::Assistant => {
                flush_user_run(&mut steps, &mut user_run);

                for part in &message.parts {
                    match part {
                        IrPart::Text { text } => {
                            steps.push(RenderStep::Typed(TypedStep::OutputText {
                                text: text.clone(),
                            }));
                        }

                        IrPart::Image { .. } | IrPart::ToolOutput { .. } => {
                            // An assistant turn has no image or tool-output slot on this family's
                            // wire: images are request content and outputs answer calls, so a
                            // misplaced part is dropped rather than rendered into a foreign step.
                        }

                        IrPart::ToolCall {
                            call_id,
                            name,
                            arguments,
                            thought_signature,
                        } => {
                            format::arguments_as_object(arguments)?;

                            steps.push(RenderStep::Typed(TypedStep::FunctionCall {
                                id: call_id.clone(),
                                name: name.clone(),
                                arguments: arguments.clone(),
                                signature: thought_signature.clone(),
                            }));
                        }

                        IrPart::Reasoning {
                            text,
                            signature,
                            opaque,
                        } => steps.push(render_thought(text, signature.clone(), opaque)),
                    }
                }
            }

            IrRole::Tool => {
                flush_user_run(&mut steps, &mut user_run);

                for part in &message.parts {
                    if let IrPart::ToolOutput {
                        call_id, content, ..
                    } = part
                    {
                        let name = resolve_tool_output_name(call_id, ir)?;

                        steps.push(RenderStep::Typed(TypedStep::FunctionResult {
                            name,
                            call_id: call_id.clone(),
                            result: vec![ContentBlock::Text {
                                text: content.clone(),
                            }],
                        }));
                    }
                }
            }
        }
    }

    flush_user_run(&mut steps, &mut user_run);

    Ok(steps)
}

/// Renders one reasoning block as a thought step: the stored step object replays verbatim when it
/// is a well-formed thought step of this family, else the block synthesizes a fresh step from its
/// text and signature and the opaque payload is dropped.
fn render_thought(text: &str, signature: Option<String>, opaque: &Value) -> RenderStep {
    if is_family_thought_step(opaque) {
        // The clone preserves every member and value of the stored step; only key order is
        // canonicalized by the map ordering — see the module docs.
        return RenderStep::Verbatim(opaque.clone());
    }

    RenderStep::Typed(TypedStep::Thought {
        id: format!("thought_{}", ids::new_uuid()),
        summary: (!text.is_empty()).then(|| text.to_string()),
        signature,
    })
}

/// Tests the family membership of an opaque replay payload: an object whose `type` is the thought
/// step tag replays onto this family's wire, anything else — cross-family blocks, text-only
/// blocks — is dropped in favor of synthesized content.
///
/// The probe is a typed struct rather than a member lookup, so the payload is modelled, never
/// walked ad hoc.
fn is_family_thought_step(opaque: &Value) -> bool {
    /// Minimal shape probe for the family-membership test.
    #[derive(Debug, Deserialize)]
    struct ThoughtProbe {
        /// Step type tag; anything but the thought tag is a foreign block.
        #[serde(rename = "type")]
        step_type: String,
    }

    serde_json::from_value::<ThoughtProbe>(opaque.clone())
        .is_ok_and(|probe| probe.step_type == STEP_THOUGHT)
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

/// Renders the advertised function tools in the typed Interactions shape.
fn render_tools(ir: &IrRequest) -> Vec<UpstreamTool> {
    ir.tools
        .iter()
        .map(|tool| UpstreamTool {
            tool_type: "function",
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
        })
        .collect()
}

/// Renders the sampling, reasoning, and tool-choice configuration; `None` when every field is
/// absent, so the whole member is omitted from the request.
fn render_generation_config(ir: &IrRequest) -> Option<GenerationConfig> {
    let tool_choice = ir
        .tool_choice
        .as_ref()
        .map(|tool_choice| match tool_choice {
            IrToolChoice::Auto => UpstreamToolChoice {
                mode: ToolChoiceMode::Auto,
                allowed_tools: None,
            },

            IrToolChoice::None => UpstreamToolChoice {
                mode: ToolChoiceMode::None,
                allowed_tools: None,
            },

            IrToolChoice::Required => UpstreamToolChoice {
                mode: ToolChoiceMode::Any,
                allowed_tools: None,
            },

            IrToolChoice::Function { name } => UpstreamToolChoice {
                mode: ToolChoiceMode::Any,
                allowed_tools: Some(vec![name.clone()]),
            },
        });

    let config = GenerationConfig {
        temperature: ir.temperature,
        top_p: ir.top_p,
        max_output_tokens: ir.max_output_tokens,
        thinking_level: ir.reasoning_effort.as_ref().map(|level| level.as_str()),
        tool_choice,
    };

    (!config.is_empty()).then_some(config)
}

/// Renders the response format restriction; free-form text renders no member at all.
fn render_response_format(ir: &IrRequest) -> Option<ResponseFormatShape> {
    match &ir.response_format {
        None | Some(IrResponseFormat::Text) => None,

        Some(IrResponseFormat::Json) => Some(ResponseFormatShape::Json {}),

        Some(IrResponseFormat::JsonSchema { name, schema }) => {
            Some(ResponseFormatShape::JsonSchema {
                name: name.clone(),
                schema: schema.clone(),
            })
        }
    }
}

/// Parses upstream Gemini Interactions responses — SSE frames or a full Interaction body — into IR
/// events, discriminating streams on the `event:` name, never on JSON members.
#[derive(Debug, Default)]
pub(crate) struct UpstreamParser {
    /// Whether [`IrEvent::MessageStart`] was emitted.
    is_started: bool,

    /// Whether a finish was already emitted; a clean end-of-stream without one degrades to
    /// [`IrFinishReason::Stop`] in the finish hook.
    is_finished: bool,

    /// Count of tool calls opened in this response; the stable per-call index source.
    call_counter: usize,

    /// Steps opened by `step.start` and not yet closed by `step.stop`, keyed by the upstream step
    /// index.
    open_steps: Vec<OpenStep>,
}

/// One upstream step tracked across stream frames.
#[derive(Debug)]
enum OpenStep {
    /// An open model text step.
    Text {
        /// Upstream step index.
        index: u64,
    },

    /// An open model reasoning block, accumulating its summary text. The step identifier needs
    /// no tracking: every stop carries its complete step, which becomes the block's opaque
    /// payload verbatim.
    Thought {
        /// Upstream step index.
        index: u64,

        /// Summary text streamed so far.
        summary: String,

        /// Signature streamed so far, when the upstream sent one mid-step.
        signature: Option<String>,
    },

    /// An open model tool invocation, accumulating its argument bytes.
    Call {
        /// Upstream step index.
        index: u64,

        /// Stable tool-call index within the response.
        call_index: usize,

        /// Identifier of the invocation.
        id: String,

        /// Name of the invoked function.
        name: String,

        /// Argument bytes streamed so far.
        arguments: String,
    },
}

impl OpenStep {
    /// The upstream step index the tracked step was opened under.
    fn index(&self) -> u64 {
        match self {
            Self::Text { index } | Self::Thought { index, .. } | Self::Call { index, .. } => *index,
        }
    }
}

/// One upstream Interaction body or envelope member: the non-streaming response shape.
///
/// Every member is optional and unmodeled members are ignored: upstreams are third-party. Steps
/// are raw values — fragments whose per-kind shape the parser models separately — so a thought
/// step's complete JSON survives as the block's opaque replay payload.
#[derive(Debug, Default, Deserialize)]
struct InteractionBody {
    /// Upstream interaction identifier, empty when the upstream omits it.
    #[serde(default)]
    id: String,

    /// Upstream model name, empty when the upstream omits it.
    #[serde(default)]
    model: String,

    /// Terminal status string, deciding the finish mapping.
    #[serde(default)]
    status: Option<String>,

    /// The interaction's steps in order.
    #[serde(default)]
    steps: Vec<Value>,

    /// Token accounting.
    #[serde(default)]
    usage: Option<UsageReport>,

    /// Failure the upstream may embed in a 2xx body.
    #[serde(default)]
    error: Option<UpstreamError>,
}

/// Minimal probe reading the failure member an upstream may embed in any frame or body.
#[derive(Debug, Deserialize)]
struct ErrorProbe {
    /// Failure the upstream attached, absent on success.
    #[serde(default)]
    error: Option<UpstreamError>,
}

/// Minimal probe reading a step's type tag; the per-kind payloads are modelled separately.
#[derive(Debug, Deserialize)]
struct StepProbe {
    /// Step type tag, empty when the step carries none.
    #[serde(rename = "type", default)]
    step_type: String,
}

/// An `output_text` step of an Interaction body or a `step.stop` frame.
#[derive(Debug, Deserialize)]
struct OutputTextStep {
    /// The text content, empty when the upstream omits it.
    #[serde(default)]
    text: String,
}

/// A `thought` step of an Interaction body or a `step.stop` frame: the summary text and the
/// terminating signature. The step identifier is deliberately unmodelled — it rides inside the
/// opaque fragment untouched, so the codec never reads it.
///
/// No unknown-field rejection: upstream payloads are third-party, and every member beyond the
/// modelled ones rides the opaque fragment untouched.
#[derive(Debug, Deserialize)]
struct ThoughtStep {
    /// Reasoning text, empty when the upstream omits it.
    #[serde(default)]
    summary: Option<String>,

    /// Upstream signature terminating the block, absent when the upstream sent none.
    #[serde(default)]
    signature: Option<String>,
}

/// A `function_call` step of an Interaction body or a `step.stop` frame.
#[derive(Debug, Deserialize)]
struct FunctionCallStep {
    /// Identifier of the invocation, empty when the upstream omits it.
    #[serde(default)]
    id: String,

    /// Name of the invoked function, empty when the upstream omits it.
    #[serde(default)]
    name: String,

    /// Arguments the model supplied, as JSON-encoded text or as the object itself.
    #[serde(default)]
    arguments: Option<StepArguments>,

    /// Upstream thought signature that must be replayed on the next turn.
    #[serde(default)]
    signature: Option<String>,
}

/// The two wire spellings of function-call arguments: the JSON-encoded text form and the object
/// form. Both normalize to the encoded-text form the IR carries.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StepArguments {
    /// The JSON-encoded text form.
    Text(String),

    /// The object form.
    Object(Map<String, Value>),
}

impl StepArguments {
    /// Normalizes either spelling to the JSON-encoded text the IR carries.
    fn into_text(self) -> String {
        match self {
            Self::Text(text) => text,

            // Rendering a JSON object map cannot fail: the value holds finite numbers only.
            Self::Object(map) => Value::Object(map).to_string(),
        }
    }
}

/// A partial step carried by `step.start`: the opening shape of a step, with only the members an
/// opening needs.
#[derive(Debug, Default, Deserialize)]
struct PartialStep {
    /// Step type tag, empty when the frame carries none.
    #[serde(rename = "type", default)]
    step_type: String,

    /// Step identifier of thought and function-call steps.
    #[serde(default)]
    id: Option<String>,

    /// Function name of a function-call step.
    #[serde(default)]
    name: Option<String>,
}

/// One incremental delta carried by `step.delta`, tagged by its kind.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum StepDelta {
    /// Model text bytes.
    #[serde(rename = "text")]
    Text {
        /// The text bytes.
        text: String,
    },

    /// Reasoning summary bytes.
    #[serde(rename = "thought_summary")]
    ThoughtSummary {
        /// The summary bytes.
        summary: String,
    },

    /// The signature terminating a reasoning block.
    #[serde(rename = "thought_signature")]
    ThoughtSignature {
        /// The signature text.
        signature: String,
    },

    /// Tool argument bytes.
    #[serde(rename = "tool_arguments")]
    ToolArguments {
        /// The argument bytes.
        arguments: String,
    },
}

/// The `step.start` frame payload.
#[derive(Debug, Deserialize)]
struct StepStartFrame {
    /// Upstream step index the frame opens.
    step_index: u64,

    /// The opening shape of the step.
    #[serde(default)]
    step: PartialStep,
}

/// The `step.delta` frame payload.
#[derive(Debug, Deserialize)]
struct StepDeltaFrame {
    /// Upstream step index the delta belongs to.
    step_index: u64,

    /// The incremental delta.
    delta: StepDelta,
}

/// The `step.stop` frame payload: the step index beside the complete step as a raw fragment, so
/// a thought step's JSON survives verbatim as the block's opaque payload.
#[derive(Debug, Deserialize)]
struct StepStopFrame {
    /// Upstream step index the frame closes.
    step_index: u64,

    /// The complete step.
    #[serde(default)]
    step: Value,
}

/// The `interaction.created` frame payload.
#[derive(Debug, Deserialize)]
struct CreatedFrame {
    /// Upstream interaction identifier, empty when the upstream omits it.
    #[serde(default)]
    id: String,

    /// Upstream model name, empty when the upstream omits it.
    #[serde(default)]
    model: String,
}

/// The `interaction.complete` frame payload.
#[derive(Debug, Deserialize)]
struct CompleteFrame {
    /// Terminal status string, deciding the finish mapping.
    #[serde(default)]
    status: Option<String>,

    /// Human-readable failure detail of a failed interaction.
    #[serde(default)]
    message: Option<String>,

    /// Token accounting.
    #[serde(default)]
    usage: Option<UsageReport>,
}

/// Upstream token accounting in the Interactions shape.
#[derive(Debug, Default, Deserialize)]
struct UsageReport {
    /// Tokens consumed by the input.
    #[serde(default)]
    total_input_tokens: u64,

    /// Tokens produced by the output, excluding reasoning.
    #[serde(default)]
    total_output_tokens: u64,

    /// Tokens spent on model reasoning.
    #[serde(default)]
    total_thought_tokens: Option<u64>,

    /// Input tokens served from the upstream cache.
    #[serde(default)]
    total_cached_tokens: Option<u64>,
}

impl UsageReport {
    /// Folds the report into IR usage: output includes reasoning (as in the GenerateContent
    /// codec), the reasoning breakdown rides beside it, and there is no cache-write accounting.
    fn into_usage(self) -> IrUsage {
        IrUsage {
            input_tokens: self.total_input_tokens,
            output_tokens: self
                .total_output_tokens
                .saturating_add(self.total_thought_tokens.unwrap_or(0)),
            cached_tokens: self.total_cached_tokens,
            cache_write_tokens: None,
            reasoning_tokens: self.total_thought_tokens,
        }
    }
}

impl UpstreamParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Emits [`IrEvent::MessageStart`] exactly once, first, with the given identity.
    fn open_events(&mut self, id: String, model: String) -> Vec<IrEvent> {
        if self.is_started {
            return Vec::new();
        }

        self.is_started = true;

        vec![IrEvent::MessageStart { id, model }]
    }

    /// Emits [`IrEvent::MessageStart`] with empty identity when no `interaction.created` arrived
    /// before content: every event stream opens with it exactly once.
    fn ensure_open(&mut self) -> Vec<IrEvent> {
        self.open_events(String::new(), String::new())
    }

    /// Removes and returns the step tracked under the upstream index, when one is open.
    fn take_open(&mut self, index: u64) -> Option<OpenStep> {
        self.open_steps
            .iter()
            .position(|open| open.index() == index)
            .map(|position| self.open_steps.swap_remove(position))
    }

    /// Returns the step tracked under the upstream index, when one is open.
    fn find_open(&mut self, index: u64) -> Option<&mut OpenStep> {
        self.open_steps
            .iter_mut()
            .find(|open| open.index() == index)
    }

    /// Ingests one complete Interaction body: message start, one event group per step, then the
    /// terminal status and usage.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for an embedded upstream error object, for steps whose kind
    /// contradicts the contract, and for a failed terminal status.
    fn ingest_body(&mut self, body: InteractionBody) -> Result<Vec<IrEvent>, PayloadError> {
        // An error object inside a 2xx body is a malformed upstream payload: the check sits
        // beside the protocol fields, shared with the other codecs.
        if let Some(error) = &body.error {
            return Err(error.failure());
        }

        let mut events = self.open_events(body.id, body.model);

        for step in body.steps {
            events.extend(self.ingest_complete_step(step)?);
        }

        events.extend(self.ingest_status(body.status, None)?);

        if let Some(usage) = body.usage {
            events.push(IrEvent::Usage(usage.into_usage()));
        }

        Ok(events)
    }

    /// Ingests one complete step value — an Interaction body member or a `step.stop` payload.
    ///
    /// Response steps deliver their content at once: text steps stream as one full delta, thought
    /// steps close as full-text done events with no prior deltas, and function calls open with a
    /// full-arguments delta plus a done — mirroring the Responses JSON path — so streaming
    /// renderers open and complete every item. Steps of other kinds (`user_input`,
    /// `function_result`, and future kinds) carry no model output and are ignored.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for a step whose kind contradicts the contract.
    fn ingest_complete_step(&mut self, step: Value) -> Result<Vec<IrEvent>, PayloadError> {
        let probe: StepProbe = format::parse_body(step.clone())?;

        match probe.step_type.as_str() {
            STEP_OUTPUT_TEXT => {
                let text: OutputTextStep = format::parse_body(step)?;

                Ok(vec![IrEvent::TextDelta(text.text)])
            }

            STEP_THOUGHT => {
                let thought: ThoughtStep = format::parse_body(step.clone())?;

                Ok(vec![IrEvent::ReasoningDone {
                    text: thought.summary.unwrap_or_default(),
                    signature: thought.signature,
                    opaque: step,
                }])
            }

            STEP_FUNCTION_CALL => {
                let call: FunctionCallStep = format::parse_body(step)?;
                let index = self.allocate_call_index();

                let arguments = call
                    .arguments
                    .map(StepArguments::into_text)
                    .unwrap_or_default();

                Ok(vec![
                    IrEvent::ToolCallDelta {
                        index,
                        call_id: Some(call.id.clone()),
                        name: Some(call.name.clone()),
                        arguments_delta: arguments.clone(),
                    },
                    IrEvent::ToolCallDone {
                        index,
                        call_id: call.id,
                        name: call.name,
                        arguments,
                        thought_signature: call.signature,
                    },
                ])
            }

            // Request-echo (`user_input`, `function_result`) and future step kinds carry no
            // model output.
            _ => Ok(Vec::new()),
        }
    }

    /// Allocates the next stable tool-call index.
    fn allocate_call_index(&mut self) -> usize {
        let index = self.call_counter;

        self.call_counter += 1;

        index
    }

    /// Ingests a terminal status string: the finish mapping, with `failed` rejected as an
    /// upstream failure. A missing status degrades to [`IrFinishReason::Stop`].
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for the failed status.
    fn ingest_status(
        &mut self,
        status: Option<String>,
        message: Option<String>,
    ) -> Result<Vec<IrEvent>, PayloadError> {
        self.is_finished = true;

        match status.as_deref() {
            Some(STATUS_REQUIRES_ACTION) => Ok(vec![IrEvent::Finish(IrFinishReason::ToolCall)]),

            Some(STATUS_INCOMPLETE) => Ok(vec![IrEvent::Finish(IrFinishReason::Length)]),

            Some(STATUS_FAILED) => Err(PayloadError::new(format!(
                "upstream interaction failed with status `failed`{}",
                message
                    .map(|detail| format!(": {detail}"))
                    .unwrap_or_default(),
            ))),

            // `completed`, a missing status, and any future status degrade to a clean stop rather
            // than failing a healthy stream.
            _ => Ok(vec![IrEvent::Finish(IrFinishReason::Stop)]),
        }
    }

    /// Ingests a `step.start` frame: records the opened step so later deltas resolve. A repeated
    /// start for an already-open index is ignored; a function-call start records the call but
    /// emits nothing — its deltas and stop carry the content.
    fn ingest_start(&mut self, frame: StepStartFrame) -> Vec<IrEvent> {
        let events = self.ensure_open();

        if self.find_open(frame.step_index).is_some() {
            return events;
        }

        match frame.step.step_type.as_str() {
            STEP_OUTPUT_TEXT => self.open_steps.push(OpenStep::Text {
                index: frame.step_index,
            }),

            STEP_THOUGHT => self.open_steps.push(OpenStep::Thought {
                index: frame.step_index,
                summary: String::new(),
                signature: None,
            }),

            STEP_FUNCTION_CALL => {
                let call_index = self.allocate_call_index();

                self.open_steps.push(OpenStep::Call {
                    index: frame.step_index,
                    call_index,
                    id: frame
                        .step
                        .id
                        .unwrap_or_else(|| format!("call_{}", ids::new_uuid())),
                    name: frame.step.name.unwrap_or_default(),
                    arguments: String::new(),
                });
            }

            // Unknown step kinds open nothing; their deltas fall into the implicit-open rule
            // below only for the known delta kinds, and their stop closes nothing.
            _ => {}
        }

        events
    }

    /// Ingests a `step.delta` frame: the delta kind decides the IR event. A delta for a step with
    /// no recorded start implicitly opens the matching kind, so a missed start cannot strand
    /// content; an implicit tool call mints its id and allocates its index the same way a start
    /// would.
    fn ingest_delta(&mut self, frame: StepDeltaFrame) -> Vec<IrEvent> {
        let mut events = self.ensure_open();

        match frame.delta {
            StepDelta::Text { text } => {
                if self.find_open(frame.step_index).is_none() {
                    self.open_steps.push(OpenStep::Text {
                        index: frame.step_index,
                    });
                }

                events.push(IrEvent::TextDelta(text));
            }

            StepDelta::ThoughtSummary { summary } => {
                if self.find_open(frame.step_index).is_none() {
                    self.open_steps.push(OpenStep::Thought {
                        index: frame.step_index,
                        summary: String::new(),
                        signature: None,
                    });
                }

                if let Some(OpenStep::Thought { summary: open, .. }) =
                    self.find_open(frame.step_index)
                {
                    open.push_str(&summary);
                }

                events.push(IrEvent::ReasoningDelta(summary));
            }

            StepDelta::ThoughtSignature { signature } => {
                if self.find_open(frame.step_index).is_none() {
                    self.open_steps.push(OpenStep::Thought {
                        index: frame.step_index,
                        summary: String::new(),
                        signature: None,
                    });
                }

                if let Some(OpenStep::Thought {
                    signature: open, ..
                }) = self.find_open(frame.step_index)
                {
                    *open = Some(signature);
                }
            }

            StepDelta::ToolArguments { arguments } => {
                if self.find_open(frame.step_index).is_none() {
                    let call_index = self.allocate_call_index();

                    self.open_steps.push(OpenStep::Call {
                        index: frame.step_index,
                        call_index,
                        id: format!("call_{}", ids::new_uuid()),
                        name: String::new(),
                        arguments: String::new(),
                    });
                }

                // A delta for a step of another kind contradicts the opened kind, so it is
                // dropped rather than misattributed to foreign content.
                if let Some(OpenStep::Call {
                    call_index,
                    id,
                    name,
                    arguments: open,
                    ..
                }) = self.find_open(frame.step_index)
                {
                    open.push_str(&arguments);

                    events.push(IrEvent::ToolCallDelta {
                        index: *call_index,
                        call_id: Some(id.clone()),
                        name: Some(name.clone()),
                        arguments_delta: arguments,
                    });
                }
            }
        }

        events
    }

    /// Ingests a `step.stop` frame carrying the complete step: closes the tracked step and emits
    /// the closing event. Thought stops close as empty-text done events — the streamed deltas
    /// delivered the text — unless no deltas preceded them, in which case the stop's own summary
    /// is the authoritative text; the step value itself becomes the block's opaque payload.
    /// Function-call stops resolve every member against the tracked state, preferring the stop's
    /// own values and falling back to the streamed ones.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for a stop step whose kind contradicts the contract.
    fn ingest_stop(&mut self, frame: StepStopFrame) -> Result<Vec<IrEvent>, PayloadError> {
        let mut events = self.ensure_open();

        let probe: StepProbe = format::parse_body(frame.step.clone())?;

        match probe.step_type.as_str() {
            STEP_OUTPUT_TEXT => {
                self.take_open(frame.step_index);
            }

            STEP_THOUGHT => {
                let thought: ThoughtStep = format::parse_body(frame.step.clone())?;
                let open = self.take_open(frame.step_index);

                let (summary, signature) = match open {
                    Some(OpenStep::Thought {
                        summary, signature, ..
                    }) => (summary, signature),

                    _ => (String::new(), Option::<String>::None),
                };

                // Deltas plus an empty-text done, or a full-text done with no deltas — never both
                // with overlapping text: streamed summary bytes make the done empty, while a stop
                // that arrives without any deltas carries the authoritative text itself.
                let text = if summary.is_empty() {
                    thought.summary.unwrap_or_default()
                } else {
                    String::new()
                };

                let signature = thought.signature.or(signature);

                events.push(IrEvent::ReasoningDone {
                    text,
                    signature,
                    opaque: frame.step,
                });
            }

            STEP_FUNCTION_CALL => {
                let call: FunctionCallStep = format::parse_body(frame.step)?;
                let open = self.take_open(frame.step_index);

                let (call_index, open_id, open_name, open_arguments, open_signature) = match open {
                    Some(OpenStep::Call {
                        call_index,
                        id,
                        name,
                        arguments,
                        ..
                    }) => (call_index, Some(id), Some(name), Some(arguments), None),

                    _ => (
                        self.allocate_call_index(),
                        Option::<String>::None,
                        Option::<String>::None,
                        Option::<String>::None,
                        Option::<String>::None,
                    ),
                };

                let arguments = call
                    .arguments
                    .map(StepArguments::into_text)
                    .or(open_arguments)
                    .unwrap_or_default();

                events.push(IrEvent::ToolCallDone {
                    index: call_index,
                    call_id: nonempty_or(call.id, open_id),
                    name: nonempty_or(call.name, open_name),
                    arguments,
                    thought_signature: call.signature.or(open_signature),
                });
            }

            // Unknown and request-echo (`user_input`, `function_result`) stops close any
            // tracked step of the index and emit nothing.
            _ => {
                self.take_open(frame.step_index);
            }
        }

        Ok(events)
    }

    /// Ingests an `interaction.complete` frame: the terminal status, then the usage report when
    /// the frame carries one.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for the failed status.
    fn ingest_complete(&mut self, frame: CompleteFrame) -> Result<Vec<IrEvent>, PayloadError> {
        let mut events = self.ensure_open();

        events.extend(self.ingest_status(frame.status, frame.message)?);

        if let Some(usage) = frame.usage {
            events.push(IrEvent::Usage(usage.into_usage()));
        }

        Ok(events)
    }
}

/// Prefers the upstream value when it is non-empty, else the tracked fallback: a stop carrying an
/// empty id or name (the upstream's unset spelling) must not clobber the streamed values.
fn nonempty_or(primary: String, fallback: Option<String>) -> String {
    if primary.is_empty() {
        fallback.unwrap_or_default()
    } else {
        primary
    }
}

impl UpstreamEventParser for UpstreamParser {
    /// Ingests one upstream SSE frame, discriminated on its `event:` name. A frame bearing a
    /// non-null `error` member aborts as an upstream failure whatever its event name; frames with
    /// an unknown or missing event name are ignored for forward compatibility.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for malformed JSON, upstream error members, and payloads whose
    /// kind contradicts the contract.
    fn ingest_frame(&mut self, frame: &SseFrame) -> Result<Vec<IrEvent>, PayloadError> {
        // The error probe is typed: any frame bearing a failure aborts, recognized event or
        // not.
        let probe: ErrorProbe = format::parse_frame(frame)?;

        if let Some(error) = probe.error {
            return Err(error.failure());
        }

        match frame.event.as_deref() {
            Some(EVENT_CREATED) => {
                let created: CreatedFrame = format::parse_frame(frame)?;

                Ok(self.open_events(created.id, created.model))
            }

            Some(EVENT_STEP_START) => {
                let start: StepStartFrame = format::parse_frame(frame)?;

                Ok(self.ingest_start(start))
            }

            Some(EVENT_STEP_DELTA) => {
                let delta: StepDeltaFrame = format::parse_frame(frame)?;

                Ok(self.ingest_delta(delta))
            }

            Some(EVENT_STEP_STOP) => {
                let stop: StepStopFrame = format::parse_frame(frame)?;

                self.ingest_stop(stop)
            }

            Some(EVENT_COMPLETE) => {
                let complete: CompleteFrame = format::parse_frame(frame)?;

                self.ingest_complete(complete)
            }

            // Heartbeats and future event kinds never surface content.
            _ => Ok(Vec::new()),
        }
    }

    /// Parses a complete upstream non-streaming Interaction body.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError`] for upstream error objects, malformed bodies, and the failed
    /// terminal status.
    fn parse_json_response(&mut self, value: Value) -> Result<Vec<IrEvent>, PayloadError> {
        let body: InteractionBody = format::parse_body(value)?;

        // A missing status degrades to a clean stop here rather than in the finish hook: the JSON
        // pump never calls it, so the body must close its own stream.
        if body.status.is_none() {
            let mut events = self.ingest_body(body)?;

            if !self.is_finished {
                self.is_finished = true;

                events.push(IrEvent::Finish(IrFinishReason::Stop));
            }

            return Ok(events);
        }

        self.ingest_body(body)
    }

    fn finish(&mut self) -> Vec<IrEvent> {
        // A clean end-of-stream that carried no finish degrades to `Stop`, so streamed and
        // aggregated renderings of the same stream agree.
        if !self.is_finished {
            self.is_finished = true;

            return vec![IrEvent::Finish(IrFinishReason::Stop)];
        }

        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes a rendered upstream body into the JSON value the tests assert against.
    ///
    /// # Panics
    ///
    /// Panics if the rendered body fails to serialize, which cannot occur for the rendered wire
    /// types.
    #[expect(
        clippy::expect_used,
        reason = "the rendered body is under test; a failing render is the failure"
    )]
    fn body_json(body: &RequestBody) -> Value {
        serde_json::to_value(body).expect("the rendered body serializes")
    }

    /// Renders one upstream body for the given IR.
    ///
    /// # Panics
    ///
    /// Panics if the IR fails to render, which the fixture prevents.
    #[expect(
        clippy::expect_used,
        reason = "the fixture IR is constructed to render"
    )]
    fn render(ir: &IrRequest) -> (String, RequestBody) {
        render_request(ir, "https://upstream.example/", "upstream-model", false)
            .expect("the fixture IR renders")
    }

    /// Verifies the request envelope: the URL pins the Interactions action, the body carries the
    /// upstream id as `model`, `store` is always false, and `stream` follows the requested mode.
    ///
    /// # Panics
    ///
    /// Panics if the envelope diverges from the contract.
    #[test]
    fn renders_the_request_envelope() {
        let ir = IrRequest {
            model: "upstream-model".to_string(),
            ..IrRequest::default()
        };

        let (url, _) = render(&ir);

        assert_eq!(url, "https://upstream.example/v1beta/interactions");

        #[expect(
            clippy::expect_used,
            reason = "the fixture IR is constructed to render"
        )]
        let (_, streaming) = render_request(&ir, "https://upstream.example", "m", true)
            .expect("the fixture IR renders");

        let rendered = body_json(&streaming);

        assert_eq!(rendered.pointer("/model"), Some(&serde_json::json!("m")));
        assert_eq!(rendered.pointer("/store"), Some(&serde_json::json!(false)));
        assert_eq!(rendered.pointer("/stream"), Some(&serde_json::json!(true)));
        assert_eq!(rendered.pointer("/input"), Some(&serde_json::json!([])));
    }

    /// Verifies the thinking slot of the closed-vocabulary contract: every vocabulary value renders
    /// into `generation_config.thinking_level` verbatim, an absent effort renders no thinking
    /// level at all, and no rendered body ever carries a token budget.
    ///
    /// # Panics
    ///
    /// Panics if a rendered slot diverges from the contract.
    #[test]
    fn renders_thinking_level_verbatim_and_never_a_budget() {
        use crate::ir::IrReasoningLevel;

        for (level, thinking_level) in [
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

            let (_, body) = render(&ir);
            let rendered = body_json(&body);

            assert_eq!(
                rendered
                    .pointer("/generation_config/thinking_level")
                    .and_then(Value::as_str),
                Some(thinking_level),
                "level `{thinking_level}` must render verbatim",
            );
            assert!(
                !rendered.to_string().contains("thinkingBudget"),
                "a token budget must never render",
            );
        }

        let ir = IrRequest {
            model: "m".to_string(),
            ..IrRequest::default()
        };

        let (_, body) = render(&ir);
        let rendered = body_json(&body);

        assert!(
            rendered
                .get("generation_config")
                .is_none_or(|config| config.get("thinking_level").is_none()),
            "an absent effort renders no thinking level",
        );
        assert!(!rendered.to_string().contains("thinkingBudget"));
    }

    /// Verifies the tool-choice mapping into `generation_config`: auto, none, required, and a
    /// forced single function (the `any` mode with one allowed tool); an absent choice omits the
    /// member.
    ///
    /// # Panics
    ///
    /// Panics if a rendered choice diverges from the mapping.
    #[test]
    fn renders_tool_choice_modes() {
        for (choice, mode, allowed) in [
            (IrToolChoice::Auto, "auto", None),
            (IrToolChoice::None, "none", None),
            (IrToolChoice::Required, "any", None),
            (
                IrToolChoice::Function {
                    name: "search".to_string(),
                },
                "any",
                Some(vec!["search"]),
            ),
        ] {
            let ir = IrRequest {
                model: "m".to_string(),
                tool_choice: Some(choice),
                ..IrRequest::default()
            };

            let (_, body) = render(&ir);
            let rendered = body_json(&body);

            let tool_choice = rendered.pointer("/generation_config/tool_choice");

            assert_eq!(
                tool_choice
                    .and_then(|choice| choice.get("mode"))
                    .and_then(Value::as_str),
                Some(mode),
            );

            match allowed {
                Some(names) => assert_eq!(
                    tool_choice.and_then(|choice| choice.get("allowed_tools")),
                    Some(&serde_json::json!(names)),
                ),

                None => assert!(
                    tool_choice.is_none_or(|choice| choice.get("allowed_tools").is_none()),
                    "a mode without a forced function renders no allowed tools",
                ),
            }
        }

        let ir = IrRequest {
            model: "m".to_string(),
            ..IrRequest::default()
        };

        let (_, body) = render(&ir);

        assert!(
            body_json(&body).pointer("/generation_config").is_none(),
            "an empty configuration is omitted entirely",
        );
    }

    /// Verifies the response-format mapping: free-form text renders no member, JSON renders the
    /// `json` shape, and a named schema rides through with its name and schema.
    ///
    /// # Panics
    ///
    /// Panics if a rendered format diverges from the mapping.
    #[test]
    fn renders_response_format() {
        let ir = IrRequest {
            model: "m".to_string(),
            response_format: Some(IrResponseFormat::JsonSchema {
                name: "answer".to_string(),
                schema: serde_json::json!({"type": "object"}),
            }),
            ..IrRequest::default()
        };

        let (_, body) = render(&ir);

        assert_eq!(
            body_json(&body).pointer("/response_format"),
            Some(&serde_json::json!({
                "type": "json_schema",
                "name": "answer",
                "schema": {"type": "object"},
            })),
        );

        let ir = IrRequest {
            model: "m".to_string(),
            response_format: Some(IrResponseFormat::Json),
            ..IrRequest::default()
        };

        let (_, body) = render(&ir);

        assert_eq!(
            body_json(&body).pointer("/response_format"),
            Some(&serde_json::json!({"type": "json"})),
        );

        let ir = IrRequest {
            model: "m".to_string(),
            ..IrRequest::default()
        };

        let (_, body) = render(&ir);

        assert!(
            body_json(&body).get("response_format").is_none(),
            "free-form text renders no response format",
        );
    }

    /// Verifies the stateless full-history replay: consecutive user turns coalesce into one
    /// `user_input` step, a same-family thought step replays its stored value exactly, a
    /// text-only reasoning block synthesizes a fresh thought step carrying its text and
    /// signature, tool calls keep their ids and signatures, and tool outputs resolve their
    /// function names into `function_result` steps.
    ///
    /// # Panics
    ///
    /// Panics if the rendered steps diverge from the pinned wire shape.
    #[test]
    fn replays_full_history_through_typed_steps() {
        use crate::ir::{IrMessage, IrRole};

        let stored_thought = serde_json::json!({
            "type": "thought",
            "id": "th_1",
            "summary": "stored reasoning",
            "signature": "sig-stored",
        });

        let ir = IrRequest {
            model: "m".to_string(),
            instructions: Some("be terse".to_string()),
            messages: vec![
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: "first".to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: "second".to_string(),
                    }],
                },
                IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![
                        IrPart::Reasoning {
                            text: "stored reasoning".to_string(),
                            signature: Some("sig-stored".to_string()),
                            opaque: stored_thought.clone(),
                        },
                        IrPart::Reasoning {
                            text: "fresh reasoning".to_string(),
                            signature: Some("sig-fresh".to_string()),
                            opaque: Value::Null,
                        },
                        IrPart::Text {
                            text: "hello".to_string(),
                        },
                        IrPart::ToolCall {
                            call_id: "call_1".to_string(),
                            name: "search".to_string(),
                            arguments: r#"{"q":"hi"}"#.to_string(),
                            thought_signature: Some("sig-call".to_string()),
                        },
                    ],
                },
                IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id: "call_1".to_string(),
                        content: "found it".to_string(),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![crate::ir::IrTool {
                name: "search".to_string(),
                description: Some("Searches.".to_string()),
                parameters: Some(serde_json::json!({"type": "object"})),
            }],
            ..IrRequest::default()
        };

        let (_, body) = render(&ir);
        let rendered = body_json(&body);

        assert_eq!(
            rendered.pointer("/system_instruction"),
            Some(&serde_json::json!("be terse")),
        );
        assert_eq!(
            rendered.pointer("/tools"),
            Some(&serde_json::json!([{
                "type": "function",
                "name": "search",
                "description": "Searches.",
                "parameters": {"type": "object"},
            }])),
        );

        // Consecutive user turns coalesce into one step.
        assert_eq!(
            rendered.pointer("/input/0"),
            Some(&serde_json::json!({
                "type": "user_input",
                "content": [{"type": "text", "text": "first"}, {"type": "text", "text": "second"}],
            })),
        );

        // The same-family thought step replays its stored value exactly.
        assert_eq!(rendered.pointer("/input/1"), Some(&stored_thought));

        // A text-only block synthesizes a fresh step carrying its text and signature.
        assert_eq!(
            rendered.pointer("/input/2/type"),
            Some(&serde_json::json!("thought"))
        );
        assert_eq!(
            rendered.pointer("/input/2/summary"),
            Some(&serde_json::json!("fresh reasoning"))
        );
        assert_eq!(
            rendered.pointer("/input/2/signature"),
            Some(&serde_json::json!("sig-fresh"))
        );

        assert_eq!(
            rendered.pointer("/input/3"),
            Some(&serde_json::json!({"type": "output_text", "text": "hello"})),
        );
        assert_eq!(
            rendered.pointer("/input/4"),
            Some(&serde_json::json!({
                "type": "function_call",
                "id": "call_1",
                "name": "search",
                "arguments": r#"{"q":"hi"}"#,
                "signature": "sig-call",
            })),
        );
        assert_eq!(
            rendered.pointer("/input/5"),
            Some(&serde_json::json!({
                "type": "function_result",
                "name": "search",
                "call_id": "call_1",
                "result": [{"type": "text", "text": "found it"}],
            })),
        );
        assert!(
            rendered.pointer("/input/6").is_none(),
            "the replay renders exactly six steps",
        );
    }

    /// Verifies the family-membership rule: a cross-family opaque payload never reaches the wire —
    /// the block synthesizes a fresh thought step from its text instead — while the signature
    /// still rides along.
    ///
    /// # Panics
    ///
    /// Panics if a foreign payload leaks onto the wire.
    #[test]
    fn drops_cross_family_opaque_payloads() {
        use crate::ir::{IrMessage, IrRole};

        let foreign = serde_json::json!({
            "type": "thinking",
            "thinking": "another family's block",
            "signature": "sig-foreign",
        });

        let ir = IrRequest {
            model: "m".to_string(),
            messages: vec![IrMessage {
                role: IrRole::Assistant,
                parts: vec![IrPart::Reasoning {
                    text: "reasoned".to_string(),
                    signature: None,
                    opaque: foreign,
                }],
            }],
            ..IrRequest::default()
        };

        let (_, body) = render(&ir);
        let rendered = body_json(&body);

        assert_eq!(
            rendered.pointer("/input/0/type"),
            Some(&serde_json::json!("thought"))
        );
        assert_eq!(
            rendered.pointer("/input/0/summary"),
            Some(&serde_json::json!("reasoned"))
        );
        assert_eq!(
            rendered.pointer("/input/0/thinking"),
            None,
            "a foreign payload must never leak onto the wire",
        );
        assert!(
            rendered.pointer("/input/1").is_none(),
            "the block renders exactly one step",
        );
    }

    /// A tool output answering an unknown call id cannot render: the function name is unresolvable
    /// and the request fails before dispatch.
    ///
    /// # Panics
    ///
    /// Panics if the rejection lands outside its documented wording.
    #[test]
    fn rejects_tool_outputs_without_a_matching_call() {
        use crate::ir::{IrMessage, IrRole};

        let ir = IrRequest {
            model: "m".to_string(),
            messages: vec![IrMessage {
                role: IrRole::Tool,
                parts: vec![IrPart::ToolOutput {
                    call_id: "call_missing".to_string(),
                    content: "orphan".to_string(),
                    is_error: false,
                }],
            }],
            ..IrRequest::default()
        };

        #[expect(
            clippy::expect_used,
            reason = "the orphan output is constructed to be rejected"
        )]
        let error = render_request(&ir, "https://upstream.example", "m", false)
            .expect_err("an orphan tool output must fail rendering");

        assert_eq!(error.param.as_deref(), Some("messages"));
        assert!(error.to_string().contains("call_missing"));
    }

    /// Builds one SSE frame of the upstream stream vocabulary.
    fn frame(event: &str, data: Value) -> SseFrame {
        SseFrame {
            data: data.to_string(),
            event: Some(event.to_string()),
        }
    }

    /// Ingests one frame of the upstream stream vocabulary on the given parser.
    ///
    /// # Panics
    ///
    /// Panics if the frame fails to parse, which the fixture prevents.
    #[expect(
        clippy::expect_used,
        reason = "the fixture frames are constructed to parse"
    )]
    fn ingest(parser: &mut UpstreamParser, event: &str, data: Value) -> Vec<IrEvent> {
        parser
            .ingest_frame(&frame(event, data))
            .expect("the fixture frame parses")
    }

    /// Opens a parser with a created frame carrying a fixed identity.
    fn ingest_created(parser: &mut UpstreamParser) {
        ingest(
            parser,
            EVENT_CREATED,
            serde_json::json!({"id": "i", "model": "m"}),
        );
    }

    /// Verifies the event-name-discriminated streaming parse end to end: created opens the stream,
    /// text deltas stream, thought summary deltas plus a signature close as an empty-text done
    /// carrying the signature and the verbatim step, tool argument deltas assemble one call, and
    /// the terminal status reports finish and usage.
    ///
    /// # Panics
    ///
    /// Panics if the parsed events diverge from the contract.
    #[test]
    fn parses_the_streaming_vocabulary_by_event_name() {
        let mut parser = UpstreamParser::new();

        let created = ingest(
            &mut parser,
            EVENT_CREATED,
            serde_json::json!({"id": "int_1", "model": "upstream-model"}),
        );

        assert_eq!(
            created,
            vec![IrEvent::MessageStart {
                id: "int_1".to_string(),
                model: "upstream-model".to_string(),
            }],
        );

        // A repeated created event is a no-op: the stream opens exactly once.
        assert!(
            ingest(
                &mut parser,
                EVENT_CREATED,
                serde_json::json!({"id": "int_2", "model": "other"}),
            )
            .is_empty()
        );

        // A text start records the step and emits nothing on its own.
        assert!(
            ingest(
                &mut parser,
                EVENT_STEP_START,
                serde_json::json!({"step_index": 0, "step": {"type": "output_text"}}),
            )
            .is_empty()
        );

        let text_delta = ingest(
            &mut parser,
            EVENT_STEP_DELTA,
            serde_json::json!({"step_index": 0, "delta": {"type": "text", "text": "hi"}}),
        );

        assert!(
            text_delta.contains(&IrEvent::TextDelta("hi".to_string())),
            "unexpected text events: {text_delta:?}",
        );

        ingest(
            &mut parser,
            EVENT_STEP_START,
            serde_json::json!({"step_index": 1, "step": {"type": "thought", "id": "th_9"}}),
        );

        ingest(
            &mut parser,
            EVENT_STEP_DELTA,
            serde_json::json!({
                "step_index": 1,
                "delta": {"type": "thought_summary", "summary": "reasoning"},
            }),
        );

        ingest(
            &mut parser,
            EVENT_STEP_DELTA,
            serde_json::json!({
                "step_index": 1,
                "delta": {"type": "thought_signature", "signature": "sig-1"},
            }),
        );

        let thought_stop = ingest(
            &mut parser,
            EVENT_STEP_STOP,
            serde_json::json!({
                "step_index": 1,
                "step": {
                    "type": "thought",
                    "id": "th_9",
                    "summary": "reasoning",
                    "signature": "sig-1",
                },
            }),
        );

        assert_eq!(thought_stop.len(), 1);
        #[expect(
            clippy::expect_used,
            reason = "the preceding length assertion guarantees one event"
        )]
        let stop = thought_stop
            .first()
            .expect("the thought stop closes with one event");
        assert!(
            matches!(stop, IrEvent::ReasoningDone { .. }),
            "a thought stop must close with a reasoning done: {thought_stop:?}",
        );

        if let IrEvent::ReasoningDone {
            text,
            signature,
            opaque,
        } = stop
        {
            // Deltas delivered the text, so the done is empty-text — never overlapping.
            assert_eq!(text, "");
            assert_eq!(signature, &Some("sig-1".to_string()));
            assert_eq!(
                opaque,
                &serde_json::json!({
                    "type": "thought",
                    "id": "th_9",
                    "summary": "reasoning",
                    "signature": "sig-1",
                }),
            );
        }

        ingest(
            &mut parser,
            EVENT_STEP_START,
            serde_json::json!({
                "step_index": 2,
                "step": {"type": "function_call", "id": "call_7", "name": "search"},
            }),
        );

        let call_delta = ingest(
            &mut parser,
            EVENT_STEP_DELTA,
            serde_json::json!({
                "step_index": 2,
                "delta": {"type": "tool_arguments", "arguments": r##"{"q":"##},
            }),
        );

        assert_eq!(
            call_delta,
            vec![IrEvent::ToolCallDelta {
                index: 0,
                call_id: Some("call_7".to_string()),
                name: Some("search".to_string()),
                arguments_delta: r##"{"q":"##.to_string(),
            }],
        );

        let call_stop = ingest(
            &mut parser,
            EVENT_STEP_STOP,
            serde_json::json!({
                "step_index": 2,
                "step": {
                    "type": "function_call",
                    "id": "call_7",
                    "name": "search",
                    "arguments": r#"{"q":"hi"}"#,
                },
            }),
        );

        assert_eq!(
            call_stop,
            vec![IrEvent::ToolCallDone {
                index: 0,
                call_id: "call_7".to_string(),
                name: "search".to_string(),
                arguments: r#"{"q":"hi"}"#.to_string(),
                thought_signature: None,
            }],
        );

        let complete = ingest(
            &mut parser,
            EVENT_COMPLETE,
            serde_json::json!({
                "status": "requires_action",
                "usage": {
                    "total_input_tokens": 10,
                    "total_output_tokens": 20,
                    "total_thought_tokens": 8,
                    "total_cached_tokens": 3,
                    "total_tool_use_tokens": 99,
                },
            }),
        );

        assert_eq!(
            complete,
            vec![
                IrEvent::Finish(IrFinishReason::ToolCall),
                IrEvent::Usage(IrUsage {
                    input_tokens: 10,
                    // Output includes reasoning; the tool-use tokens are dropped.
                    output_tokens: 28,
                    cached_tokens: Some(3),
                    cache_write_tokens: None,
                    reasoning_tokens: Some(8),
                }),
            ],
        );

        assert!(parser.finish().is_empty(), "no spurious Stop may follow");
    }

    /// A thought stop that arrives without any prior summary deltas closes as a full-text done:
    /// the stop's own summary is authoritative, so its text is never lost.
    ///
    /// # Panics
    ///
    /// Panics if the stop-only block loses its text.
    #[test]
    fn closes_a_delta_less_thought_stop_with_its_own_text() {
        let mut parser = UpstreamParser::new();

        ingest_created(&mut parser);

        let stop = ingest(
            &mut parser,
            EVENT_STEP_STOP,
            serde_json::json!({
                "step_index": 5,
                "step": {"type": "thought", "id": "th_5", "summary": "whole"},
            }),
        );

        assert_eq!(stop.len(), 1);
        #[expect(
            clippy::expect_used,
            reason = "the preceding length assertion guarantees one event"
        )]
        let only = stop.first().expect("the stop closes with one event");
        assert!(
            matches!(only, IrEvent::ReasoningDone { .. }),
            "a thought stop must close with a reasoning done: {stop:?}",
        );

        if let IrEvent::ReasoningDone { text, opaque, .. } = only {
            assert_eq!(text, "whole");
            assert_eq!(
                opaque,
                &serde_json::json!({"type": "thought", "id": "th_5", "summary": "whole"}),
            );
        }
    }

    /// Verifies the non-streaming parse: the Interaction body's steps deliver text deltas,
    /// full-text thought dones with no prior deltas, and full-arguments call pairs, then the
    /// status and usage close the stream.
    ///
    /// # Panics
    ///
    /// Panics if the parsed events diverge from the contract.
    #[test]
    fn parses_a_non_streaming_interaction_body() {
        let body = serde_json::json!({
            "id": "int_3",
            "model": "upstream-model",
            "status": "completed",
            "steps": [
                {"type": "output_text", "text": "hello"},
                {"type": "thought", "id": "th_1", "summary": "considered", "signature": "sig-t"},
                {"type": "function_call", "id": "call_1", "name": "search", "arguments": {
                    "q": "hi",
                }},
                {"type": "user_input", "content": []},
            ],
            "usage": {
                "total_input_tokens": 7,
                "total_output_tokens": 9,
            },
        });

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the fixture body is constructed to parse"
        )]
        let events = parser
            .parse_json_response(body)
            .expect("the interaction body parses");

        assert_eq!(
            events,
            vec![
                IrEvent::MessageStart {
                    id: "int_3".to_string(),
                    model: "upstream-model".to_string(),
                },
                IrEvent::TextDelta("hello".to_string()),
                IrEvent::ReasoningDone {
                    text: "considered".to_string(),
                    signature: Some("sig-t".to_string()),
                    opaque: serde_json::json!({
                        "type": "thought",
                        "id": "th_1",
                        "summary": "considered",
                        "signature": "sig-t",
                    }),
                },
                IrEvent::ToolCallDelta {
                    index: 0,
                    call_id: Some("call_1".to_string()),
                    name: Some("search".to_string()),
                    arguments_delta: r#"{"q":"hi"}"#.to_string(),
                },
                IrEvent::ToolCallDone {
                    index: 0,
                    call_id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: r#"{"q":"hi"}"#.to_string(),
                    thought_signature: None,
                },
                IrEvent::Finish(IrFinishReason::Stop),
                IrEvent::Usage(IrUsage {
                    input_tokens: 7,
                    output_tokens: 9,
                    cached_tokens: None,
                    cache_write_tokens: None,
                    reasoning_tokens: None,
                }),
            ],
        );
    }

    /// Verifies the terminal status mapping: `incomplete` finishes with length, `failed` aborts
    /// as an upstream failure on both the streaming and the buffered paths, and a stream that
    /// ends cleanly without any status degrades to stop.
    ///
    /// # Panics
    ///
    /// Panics if a status maps outside its documented finish.
    #[test]
    fn maps_terminal_statuses() {
        let mut parser = UpstreamParser::new();

        let events = ingest(
            &mut parser,
            EVENT_COMPLETE,
            serde_json::json!({"status": "incomplete"}),
        );

        assert_eq!(
            events.last(),
            Some(&IrEvent::Finish(IrFinishReason::Length))
        );

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the failed status is constructed to be rejected"
        )]
        let error = parser
            .ingest_frame(&frame(
                EVENT_COMPLETE,
                serde_json::json!({"status": "failed", "message": "crashed"}),
            ))
            .expect_err("a failed status must abort the stream");

        assert!(error.to_string().contains("failed"));
        assert!(error.to_string().contains("crashed"));

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the failed body is constructed to be rejected"
        )]
        let error = parser
            .parse_json_response(serde_json::json!({"status": "failed"}))
            .expect_err("a failed body must abort");

        assert!(error.to_string().contains("failed"));

        let mut parser = UpstreamParser::new();

        ingest_created(&mut parser);

        assert_eq!(
            parser.finish(),
            vec![IrEvent::Finish(IrFinishReason::Stop)],
            "a clean end without a status degrades to stop",
        );
    }

    /// An upstream failure embedded in a success transport aborts the same way on both paths: a
    /// frame bearing a non-null `error` member and a JSON body carrying one both fail, while
    /// frames with unknown or missing event names are ignored for forward compatibility.
    ///
    /// # Panics
    ///
    /// Panics if an embedded error parses as a success or a benign frame fails.
    #[test]
    fn rejects_embedded_upstream_errors_and_ignores_unknown_events() {
        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the error frame is constructed to be rejected"
        )]
        let error = parser
            .ingest_frame(&frame(
                EVENT_STEP_DELTA,
                serde_json::json!({"error": {"message": "overloaded"}}),
            ))
            .expect_err("an embedded error must abort the stream");

        assert_eq!(error.to_string(), "overloaded");

        let mut parser = UpstreamParser::new();

        #[expect(
            clippy::expect_used,
            reason = "the error body is constructed to be rejected"
        )]
        let error = parser
            .parse_json_response(serde_json::json!({"error": "quota exhausted"}))
            .expect_err("an embedded error must abort the body");

        assert_eq!(error.to_string(), "quota exhausted");

        let mut parser = UpstreamParser::new();

        assert!(
            ingest(&mut parser, "interaction.ping", serde_json::json!({"t": 1})).is_empty(),
            "an unknown event carries no content",
        );

        #[expect(
            clippy::expect_used,
            reason = "the eventless frame is constructed to parse"
        )]
        let eventless = parser
            .ingest_frame(&SseFrame {
                data: serde_json::json!({"t": 1}).to_string(),
                event: None,
            })
            .expect("an eventless frame parses");

        assert!(
            eventless.is_empty(),
            "an eventless frame carries no content"
        );
    }
}
