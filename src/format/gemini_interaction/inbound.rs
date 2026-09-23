//! Interactions inbound surface: the outward `POST /interactions` client format.
//!
//! This module owns the surface's request parser ([`Format::parse_request`]) and its client
//! renderer (native SSE plus aggregated responses), both speaking the wire vocabulary the parent
//! module fixes. The endpoint handler lives in
//! [`crate::api::gemini_v1beta::interactions`]; shared prefix concerns (authentication, 404/405
//! bodies, body limit, accounting row id) live beside it.
//!
//! # Surface contract
//!
//! - The model rides the request **body** (`model`): `POST /interactions` has no URL model slot, so
//!   the surface addresses models in the body like the OpenAI surfaces do. The router threads
//!   [`RouteContext`](crate::format::RouteContext) into parsing carrying only the streaming
//!   override: `?alt=sse` forces streaming, otherwise the body's `stream` member decides.
//! - The body is parsed strictly: unknown members are 400s, and the known-but-unsupported controls
//!   each reject naming their field — `previous_interaction_id`, `background`, and `last_event_id`
//!   (the gateway never chains); `thinkingBudget` and `budget_tokens` (legacy numeric mechanisms);
//!   `stop` and `stop_sequences`; `seed` and `modalities`.
//! - Presentation-only controls are accepted and ignored: `store` (in whatever shape it arrives)
//!   and `thinking_summaries`.
//! - Reasoning enters the IR through `thought` steps, whose complete step JSON is stored verbatim
//!   as the block's opaque replay payload; cross-family blocks never originate here. Tool calls
//!   replay through `function_call` steps carrying their ids and signatures.
//! - `function_result` steps accept a plain string, a text-block array (image blocks are a 400 —
//!   the IR tool output has no media channel), or the tool's own result object, flattened to JSON
//!   text. There is no `is_error` member here: that flag belongs to the Anthropic pair alone.
//! - `tool_choice` accepts the `auto`/`any`/`none` modes as a bare string or as an object with a
//!   single `allowed_tools` entry; `validated` and multiple allowed tools are 400s, as is a choice
//!   without advertised tools. Built-in/server-side tools are 400s — function tools only.
//! - Streaming responses reuse the family's own event vocabulary (`interaction.created`,
//!   `step.start`, `step.delta`, `step.stop`, `interaction.complete`) with `event:` names; the
//!   aggregated response is the Interaction resource (`id`, `model`, `status`, `steps`, `usage`). A
//!   `step.stop` always carries the complete step, so receivers replace — never append —
//!   accumulated delta state. Steps render in a fixed order the aggregation can guarantee: thought
//!   steps first, then the text step, then function calls.

use super::{
    RenderStep, ResponseFormatShape, STATUS_COMPLETED, STATUS_INCOMPLETE, STATUS_REQUIRES_ACTION,
};
use crate::{
    format::{self, ClientFormat, ClientSseRenderer, ParsedRequest, RouteContext},
    ids,
    ir::{
        InvalidRequest, IrAggregation, IrFinishReason, IrImage, IrPart, IrReasoningLevel,
        IrRequest, IrResponseFormat, IrRole, IrTool, IrToolChoice, IrUsage,
    },
    sse,
    storage::api_logs::ApiLogEndpointType,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Marker of the outward Interactions client format.
#[derive(Debug)]
pub(crate) struct Format;

impl ClientFormat for Format {
    type JsonResponse = JsonResponse;

    const ENDPOINT_TYPE: ApiLogEndpointType = ApiLogEndpointType::GeminiInteractions;

    fn parse_request(body: &[u8], route: &RouteContext) -> Result<ParsedRequest, InvalidRequest> {
        parse_request(body, route)
    }

    fn sse_renderer(
        _should_include_usage: bool,
        public_id: String,
    ) -> Box<dyn ClientSseRenderer + Send> {
        Box::new(SseRenderer::new(public_id))
    }

    fn render_json_response(public_id: &str, aggregation: &IrAggregation) -> Self::JsonResponse {
        render_json_response(public_id, aggregation)
    }
}

/// Strictly parsed Interactions request body.
///
/// Unknown members are rejected outright; the known-but-unsupported controls below are modelled
/// explicitly so their rejections name the offending field instead of rendering serde's generic
/// unknown-field wording.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestBody {
    /// Public model id to route; the Interactions API addresses models in the body.
    model: String,

    /// Conversation input as typed steps.
    input: Vec<StepIn>,

    /// System instructions.
    #[serde(default)]
    system_instruction: Option<String>,

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

    /// Reasoning level in the closed shared vocabulary.
    #[serde(default)]
    thinking_level: Option<String>,

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

    /// Summary verbosity hint; the IR has no summary channel, so it is accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    thinking_summaries: Option<serde::de::IgnoredAny>,

    /// Chained interaction resume; the gateway never chains, so a present member is a 400.
    #[serde(default)]
    previous_interaction_id: Option<serde::de::IgnoredAny>,

    /// Background execution; the gateway never chains, so a present member is a 400.
    #[serde(default)]
    background: Option<serde::de::IgnoredAny>,

    /// Resume cursor; the gateway never chains, so a present member is a 400.
    #[serde(default)]
    last_event_id: Option<serde::de::IgnoredAny>,

    /// Legacy numeric reasoning budget; a present member is a 400 naming the field. The member
    /// keeps its camel-case wire spelling through the rename.
    #[serde(default, rename = "thinkingBudget")]
    thinking_budget: Option<serde::de::IgnoredAny>,

    /// Legacy numeric reasoning budget under its alternate spelling; a present member is a 400
    /// naming the field.
    #[serde(default)]
    budget_tokens: Option<serde::de::IgnoredAny>,

    /// Stop sequences; known-unsupported, so a present member is a 400.
    #[serde(default)]
    stop: Option<serde::de::IgnoredAny>,

    /// Stop sequences under the plural spelling; a present member is a 400.
    #[serde(default)]
    stop_sequences: Option<serde::de::IgnoredAny>,

    /// Sampling seed; out of scope, so a present member is a 400.
    #[serde(default)]
    seed: Option<serde::de::IgnoredAny>,

    /// Modality configuration; out of scope, so a present member is a 400.
    #[serde(default)]
    modalities: Option<serde::de::IgnoredAny>,
}

/// One input step, tagged by `type`.
///
/// The shapes are strict: a member outside the step's own shape is rejected. The gateway's own
/// rendered responses replay through these same shapes — `output_text` and `thought` steps it
/// emitted, `function_call` steps carrying the ids and signatures it minted — so replaying them
/// verbatim parses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum StepIn {
    /// A user turn.
    UserInput {
        /// The turn's content blocks in order.
        content: Vec<ContentBlockIn>,
    },

    /// A model text turn, replayed from an earlier response.
    OutputText {
        /// The text content.
        text: String,
    },

    /// A model reasoning block: its summary text plus the signature terminating it. The complete
    /// step JSON becomes the block's opaque replay payload, so the step replays byte-exact to
    /// the Interactions upstream.
    Thought {
        /// Step identifier, round-tripped verbatim.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,

        /// Reasoning text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,

        /// Upstream signature terminating the block.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// A past tool invocation, carrying its id and signature for cross-turn matching.
    FunctionCall {
        /// Identifier of the invocation, round-tripped to the matching tool output.
        id: String,

        /// Name of the invoked function.
        name: String,

        /// JSON-encoded arguments, or the arguments object itself.
        arguments: CallArgumentsIn,

        /// Upstream thought signature to reattach across turns.
        #[serde(default)]
        signature: Option<String>,
    },

    /// A tool result for a prior function call.
    FunctionResult {
        /// Name of the function being answered.
        name: String,

        /// Identifier of the invocation being answered.
        call_id: String,

        /// Result content: a plain string, a text-block array, or the tool's own result object
        /// flattened to JSON text.
        result: FunctionResultPayload,
    },
}

/// One content block of a `user_input` step or a `function_result` block array, tagged by `type`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ContentBlockIn {
    /// Plain text.
    Text {
        /// The text content.
        text: String,
    },

    /// Image content. The URL is optional in the shape so the conversion — not serde's
    /// shape check — owns the rejection: images are accepted in user turns and rejected in tool
    /// results, and both rejections must name their field.
    Image {
        /// Remote or data URL of the image; required in user turns.
        #[serde(default)]
        image_url: Option<String>,
    },
}

/// The two accepted spellings of function-call arguments: the JSON-encoded text form and the
/// arguments object itself. Both normalize to the encoded-text form the IR carries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum CallArgumentsIn {
    /// The JSON-encoded text form.
    Text(String),

    /// The arguments object form.
    Object(Map<String, Value>),
}

impl CallArgumentsIn {
    /// Normalizes either spelling to the JSON-encoded text the IR carries, validating that the
    /// text encodes an object — in parity with the OpenAI surfaces, which validate tool-call
    /// arguments at parse time.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRequest`] naming `arguments` for text that is not a JSON object.
    fn into_text(self) -> Result<String, InvalidRequest> {
        match self {
            Self::Text(text) => {
                format::arguments_as_object(&text)?;

                Ok(text)
            }

            Self::Object(map) => Ok(format::arguments_object_to_text(map)),
        }
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
                let mut combined = String::new();

                for block in blocks {
                    match block {
                        ContentBlockIn::Text { text } => {
                            if !combined.is_empty() {
                                combined.push_str("\n\n");
                            }

                            combined.push_str(&text);
                        }

                        ContentBlockIn::Image { .. } => {
                            return Err(InvalidRequest::field(
                                "result",
                                "function_result content blocks must be text; \
                                 `image` is not supported",
                            ));
                        }
                    }
                }

                Ok(combined)
            }

            // Rendering a JSON object map cannot fail: the value holds finite numbers only.
            Self::Object(map) => Ok(Value::Object(map).to_string()),
        }
    }
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

/// Tool choice: a bare mode name, or an object pairing a mode with a single allowed function.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ToolChoiceIn {
    /// The bare mode form (`auto`, `any`, `none`).
    Mode(String),

    /// The object form: a mode with an optional single allowed function.
    Object {
        /// Selection mode.
        mode: String,

        /// The one function the model may call.
        #[serde(default)]
        allowed_tools: Option<Vec<String>>,
    },
}

/// Parses an Interactions request body into IR with the router's route context: the URL carries
/// no model on this surface, so the body must name it; only the streaming mode may arrive from
/// the route (`?alt=sse` overriding the body's `stream` member).
///
/// # Errors
///
/// Returns [`InvalidRequest`] for malformed or unsupported requests.
fn parse_request(body: &[u8], route: &RouteContext) -> Result<ParsedRequest, InvalidRequest> {
    let request: RequestBody = format::parse_client_request(body)?;

    reject_present_fields(&request)?;
    format::validate_sampling_ranges(request.temperature, request.top_p)?;

    // Consecutive same-role steps coalesce into one message per run, so a text turn followed by
    // its thought step and tool calls stays one assistant message.
    let mut run = format::RoleRun::default();

    for step in request.input {
        match step {
            StepIn::UserInput { content } => {
                let mut parts = Vec::with_capacity(content.len());

                for block in content {
                    match block {
                        ContentBlockIn::Text { text } => {
                            parts.push(IrPart::Text { text });
                        }

                        ContentBlockIn::Image { image_url } => {
                            let Some(image_url) = image_url else {
                                return Err(InvalidRequest::field(
                                    "content",
                                    "user_input image blocks require an `image_url`",
                                ));
                            };

                            parts.push(IrPart::Image {
                                image: IrImage::from_url(&image_url)?,
                            });
                        }
                    }
                }

                run.push(IrRole::User, parts);
            }

            StepIn::OutputText { text } => {
                run.push(IrRole::Assistant, vec![IrPart::Text { text }]);
            }

            StepIn::Thought {
                id,
                summary,
                signature,
            } => {
                // The opaque payload is the step's own complete JSON: strict parsing guarantees
                // no member escapes the typed shape, so re-serializing it preserves the value
                // exactly. Serialization of the parsed shape is total.
                let opaque = serde_json::to_value(StepIn::Thought {
                    id,
                    summary: summary.clone(),
                    signature: signature.clone(),
                })
                .unwrap_or(Value::Null);

                run.push(
                    IrRole::Assistant,
                    vec![IrPart::Reasoning {
                        text: summary.unwrap_or_default(),
                        signature,
                        opaque,
                    }],
                );
            }

            StepIn::FunctionCall {
                id,
                name,
                arguments,
                signature,
            } => {
                run.push(
                    IrRole::Assistant,
                    vec![IrPart::ToolCall {
                        call_id: id,
                        name,
                        arguments: arguments.into_text()?,
                        thought_signature: signature,
                    }],
                );
            }

            StepIn::FunctionResult {
                // The name is accepted for wire compatibility but not preserved: the IR tool
                // output carries no name, and the upstream render resolves it from the matching
                // call instead.
                name: _,
                call_id,
                result,
            } => {
                run.push(
                    IrRole::Tool,
                    vec![IrPart::ToolOutput {
                        call_id,
                        content: result.into_text()?,
                        is_error: false,
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

    let tool_choice = match &request.tool_choice {
        None => None,

        Some(choice) => {
            format::reject_tool_choice_without_tools(tools.is_empty())?;

            Some(parse_tool_choice(choice)?)
        }
    };

    let response_format = match request.response_format {
        None => None,
        Some(ResponseFormatShape::Text {}) => Some(IrResponseFormat::Text),
        Some(ResponseFormatShape::Json {}) => Some(IrResponseFormat::Json),

        Some(ResponseFormatShape::JsonSchema { name, schema }) => {
            Some(IrResponseFormat::JsonSchema { name, schema })
        }
    };

    let reasoning_effort = request
        .thinking_level
        .map(|level| IrReasoningLevel::parse(&level, "thinking_level"))
        .transpose()?;

    Ok(ParsedRequest {
        ir: IrRequest {
            // The model rides the body on this surface: `POST /interactions` has no URL model
            // slot, so the route context carries no public id and the body's member decides.
            model: request.model,
            instructions: request.system_instruction,
            messages,
            tools,
            tool_choice,
            temperature: request.temperature,
            top_p: request.top_p,
            max_output_tokens: request.max_output_tokens,
            response_format,
            reasoning_effort,
        },
        // The `?alt=sse` query forces streaming; otherwise the body's `stream` member decides.
        is_streaming: route
            .is_streaming
            .unwrap_or(request.stream.unwrap_or(false)),
        // Usage is always included on this surface; only Chat Completions exposes the option.
        should_include_usage: false,
    })
}

/// Rejects every known-but-unsupported control the request carries, each naming its own field.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming the first present control.
fn reject_present_fields(request: &RequestBody) -> Result<(), InvalidRequest> {
    // Each control rejects with the wording its disposition row documents: chaining controls name
    // the statelessness invariant, numeric budgets name the level vocabulary that replaces them.
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

    if request.last_event_id.is_some() {
        return Err(InvalidRequest::field(
            "last_event_id",
            "The gateway is stateless; last_event_id is not supported",
        ));
    }

    if request.thinking_budget.is_some() {
        return Err(InvalidRequest::field(
            "thinkingBudget",
            "thinkingBudget is not supported; use thinking_level",
        ));
    }

    if request.budget_tokens.is_some() {
        return Err(InvalidRequest::field(
            "budget_tokens",
            "budget_tokens is not supported; use thinking_level",
        ));
    }

    if request.stop.is_some() {
        return Err(InvalidRequest::field("stop", "stop is not supported"));
    }

    if request.stop_sequences.is_some() {
        return Err(InvalidRequest::field(
            "stop_sequences",
            "stop_sequences is not supported",
        ));
    }

    if request.seed.is_some() {
        return Err(InvalidRequest::field("seed", "seed is not supported"));
    }

    if request.modalities.is_some() {
        return Err(InvalidRequest::field(
            "modalities",
            "modalities is not supported",
        ));
    }

    Ok(())
}

/// Parses the `tool_choice` member into IR: the `auto`/`any`/`none` modes, with `any` plus one
/// allowed tool forcing that function. `validated` and multiple allowed tools are 400s.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `tool_choice` for unknown or unsupported choices.
fn parse_tool_choice(choice: &ToolChoiceIn) -> Result<IrToolChoice, InvalidRequest> {
    let (mode, allowed_tools) = match choice {
        ToolChoiceIn::Mode(mode) => (mode.as_str(), None),

        ToolChoiceIn::Object {
            mode,
            allowed_tools,
        } => (mode.as_str(), allowed_tools.as_ref()),
    };

    match mode {
        "auto" => {
            reject_allowed_tools(allowed_tools, "auto")?;

            Ok(IrToolChoice::Auto)
        }

        "any" => match allowed_tools {
            None => Ok(IrToolChoice::Required),

            Some(names) => match names.as_slice() {
                [] => Ok(IrToolChoice::Required),

                [name] => Ok(IrToolChoice::Function { name: name.clone() }),

                _ => Err(InvalidRequest::field(
                    "tool_choice",
                    "tool_choice allows at most one entry in allowed_tools",
                )),
            },
        },

        "none" => {
            reject_allowed_tools(allowed_tools, "none")?;

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

/// Rejects an `allowed_tools` list paired with a mode that takes none.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `tool_choice` when the list is present and non-empty.
fn reject_allowed_tools(
    allowed_tools: Option<&Vec<String>>,
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

/// The non-streaming success body: the Interaction resource over the aggregated events.
#[derive(Debug, Serialize)]
pub(crate) struct JsonResponse {
    /// Renderer-minted interaction identifier.
    id: String,

    /// Public routing id the client addressed.
    model: String,

    /// Terminal status derived from the aggregated finish.
    status: &'static str,

    /// Response steps: thought steps, then the text step, then function calls.
    steps: Vec<RenderStep>,

    /// Token accounting, absent when the upstream reported no usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<OutUsage>,
}

/// Token accounting in the Interactions usage shape: the inverse of the upstream usage mapping —
/// the IR output total (which includes reasoning) splits back into the output and thought
/// members, so a same-family round-trip restores the reported numbers.
#[derive(Debug, Serialize)]
struct OutUsage {
    /// Tokens consumed by the input.
    total_input_tokens: u64,

    /// Tokens produced by the output, excluding reasoning.
    total_output_tokens: u64,

    /// Tokens spent on model reasoning, omitted when the upstream reported none.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_thought_tokens: Option<u64>,

    /// Input tokens served from the upstream cache, omitted when the upstream reported none.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_cached_tokens: Option<u64>,
}

impl OutUsage {
    /// Splits an IR usage report back into the Interactions members.
    fn from_usage(usage: &IrUsage) -> Self {
        Self {
            total_input_tokens: usage.input_tokens,
            total_output_tokens: usage
                .output_tokens
                .saturating_sub(usage.reasoning_tokens.unwrap_or(0)),
            total_thought_tokens: usage.reasoning_tokens,
            total_cached_tokens: usage.cached_tokens,
        }
    }
}

/// Renders an aggregated completion as the non-streaming Interaction resource. Steps render in the
/// one order the aggregation can guarantee — thought steps in block order, then the text step,
/// then function calls — because the fold does not preserve the original interleaving.
///
/// # Panics
///
/// Panics if serializing a stored opaque step fails, which cannot occur for JSON values.
fn render_json_response(public_id: &str, aggregation: &IrAggregation) -> JsonResponse {
    let mut steps: Vec<RenderStep> = Vec::new();

    for block in &aggregation.reasoning_blocks {
        steps.push(super::render_thought(
            &block.text,
            block.signature.clone(),
            &block.opaque,
        ));
    }

    if !aggregation.text.is_empty() {
        steps.push(RenderStep::Typed(super::TypedStep::OutputText {
            text: aggregation.text.clone(),
        }));
    }

    for call in &aggregation.tool_calls {
        steps.push(RenderStep::Typed(super::TypedStep::FunctionCall {
            id: call.call_id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            signature: call.thought_signature.clone(),
        }));
    }

    JsonResponse {
        id: format!("int_{}", ids::new_uuid()),
        model: public_id.to_string(),
        status: finish_status(aggregation.finish),
        steps,
        usage: aggregation.usage.as_ref().map(OutUsage::from_usage),
    }
}

/// Maps the canonical finish onto the Interactions status vocabulary. The vocabulary has no
/// content-filter member: a filtered turn was cut off, so it reports `incomplete` — the same
/// bucket the Responses surface renders it into.
fn finish_status(finish: Option<IrFinishReason>) -> &'static str {
    match finish {
        None | Some(IrFinishReason::Stop) => STATUS_COMPLETED,
        Some(IrFinishReason::ToolCall) => STATUS_REQUIRES_ACTION,
        Some(IrFinishReason::Length) | Some(IrFinishReason::ContentFilter) => STATUS_INCOMPLETE,
    }
}

/// Streaming renderer emitting the family's own event vocabulary with `event:` names.
#[derive(Debug)]
struct SseRenderer {
    /// Public routing id the client addressed, echoed in the created frame.
    public_id: String,

    /// Renderer-minted interaction identifier.
    interaction_id: String,

    /// Next upstream step index to mint for an opened step.
    next_step_index: u64,

    /// Whether the created frame was emitted.
    is_created: bool,

    /// Open model text step, accumulating its text.
    open_text: Option<OpenTextStep>,

    /// Open model reasoning block, accumulating its summary.
    open_thought: Option<OpenThoughtStep>,

    /// Open model tool invocations by call index.
    open_calls: Vec<OpenCallStep>,

    /// Terminal finish buffered for the complete frame.
    finish: Option<IrFinishReason>,

    /// Terminal usage buffered for the complete frame.
    usage: Option<IrUsage>,

    /// Whether the complete frame was emitted; the gateway sends `Done` exactly once, so this is
    /// a defensive guard, not a second terminal path.
    is_complete: bool,
}

/// An open model text step of the client stream.
#[derive(Debug)]
struct OpenTextStep {
    /// Minted step index.
    index: u64,

    /// Text streamed so far.
    text: String,
}

/// An open model reasoning block of the client stream.
#[derive(Debug)]
struct OpenThoughtStep {
    /// Minted step index.
    index: u64,

    /// Minted step identifier.
    id: String,

    /// Summary streamed so far.
    summary: String,
}

/// An open model tool invocation of the client stream.
#[derive(Debug)]
struct OpenCallStep {
    /// Stable tool-call index within the response.
    call_index: usize,

    /// Minted step index.
    step_index: u64,

    /// Identifier of the invocation, filled in as deltas reveal it.
    id: String,

    /// Name of the invoked function, filled in as deltas reveal it.
    name: String,
}

/// The `interaction.created` frame payload.
#[derive(Debug, Serialize)]
struct CreatedPayload<'a> {
    /// Renderer-minted interaction identifier.
    id: &'a str,

    /// Public routing id the client addressed.
    model: &'a str,
}

/// The `step.start` frame payload.
#[derive(Debug, Serialize)]
struct StartPayload<'a> {
    /// Step index the frame opens.
    step_index: u64,

    /// The opening shape of the step.
    step: StartStep<'a>,
}

/// The opening shape of a streamed step: its kind, with the id and name when already known.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum StartStep<'a> {
    /// An opening text step.
    #[serde(rename = "output_text")]
    OutputText {},

    /// An opening reasoning block.
    #[serde(rename = "thought")]
    Thought {
        /// Step identifier.
        id: &'a str,
    },

    /// An opening tool invocation.
    #[serde(rename = "function_call")]
    FunctionCall {
        /// Identifier of the invocation, empty until deltas reveal it.
        id: &'a str,

        /// Name of the invoked function, empty until deltas reveal it.
        name: &'a str,
    },
}

/// The `step.delta` frame payload.
#[derive(Debug, Serialize)]
struct DeltaPayload<'a> {
    /// Step index the delta belongs to.
    step_index: u64,

    /// The incremental delta.
    delta: OutDelta<'a>,
}

/// One incremental delta of the client stream, tagged by its kind.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum OutDelta<'a> {
    /// Model text bytes.
    #[serde(rename = "text")]
    Text {
        /// The text bytes.
        text: &'a str,
    },

    /// Reasoning summary bytes.
    #[serde(rename = "thought_summary")]
    ThoughtSummary {
        /// The summary bytes.
        summary: &'a str,
    },

    /// The signature terminating a reasoning block.
    #[serde(rename = "thought_signature")]
    ThoughtSignature {
        /// The signature text.
        signature: &'a str,
    },

    /// Tool argument bytes.
    #[serde(rename = "tool_arguments")]
    ToolArguments {
        /// The argument bytes.
        arguments: &'a str,
    },
}

/// The `step.stop` frame payload: the index beside the complete step, so receivers replace —
/// never append — accumulated delta state.
#[derive(Debug, Serialize)]
struct StopPayload<'a> {
    /// Step index the frame closes.
    step_index: u64,

    /// The complete step.
    step: &'a RenderStep,
}

/// The `interaction.complete` frame payload.
#[derive(Debug, Serialize)]
struct CompletePayload<'a> {
    /// Terminal status derived from the finish.
    status: &'static str,

    /// Token accounting, absent when the stream reported no usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<OutUsageRef<'a>>,
}

/// Token accounting borrowed for the complete frame.
#[derive(Debug, Serialize)]
struct OutUsageRef<'a> {
    /// Tokens consumed by the input.
    total_input_tokens: u64,

    /// Tokens produced by the output, excluding reasoning.
    total_output_tokens: u64,

    /// Tokens spent on model reasoning, omitted when the stream reported none.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_thought_tokens: Option<u64>,

    /// Input tokens served from the upstream cache, omitted when the stream reported none.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_cached_tokens: Option<&'a u64>,
}

impl SseRenderer {
    /// Boxes a renderer for one parsed request over the public routing id the client addressed.
    fn new(public_id: String) -> Self {
        Self {
            public_id,
            interaction_id: format!("int_{}", ids::new_uuid()),
            next_step_index: 0,
            is_created: false,
            open_text: None,
            open_thought: None,
            open_calls: Vec::new(),
            finish: None,
            usage: None,
            is_complete: false,
        }
    }

    /// Emits the created frame exactly once, first, ahead of any other frame.
    fn ensure_created(&mut self) -> Vec<String> {
        if self.is_created {
            return Vec::new();
        }

        self.is_created = true;

        vec![sse::format_frame(
            Some(super::EVENT_CREATED),
            &format::render_json(&CreatedPayload {
                id: &self.interaction_id,
                model: &self.public_id,
            }),
        )]
    }

    /// Opens the text step when none is open, returning its index.
    fn open_text_step(&mut self, frames: &mut Vec<String>) -> u64 {
        if let Some(open) = &self.open_text {
            return open.index;
        }

        let index = self.next_step_index;

        self.next_step_index += 1;
        self.open_text = Some(OpenTextStep {
            index,
            text: String::new(),
        });

        frames.push(sse::format_frame(
            Some(super::EVENT_STEP_START),
            &format::render_json(&StartPayload {
                step_index: index,
                step: StartStep::OutputText {},
            }),
        ));

        index
    }

    /// Opens the reasoning block under the given id when none is open, returning its index.
    /// Callers pass the stored id for done-only blocks and a minted one for live deltas, so the
    /// opened step always carries the id its stop will close with.
    fn open_thought_step(&mut self, frames: &mut Vec<String>, id: String) -> u64 {
        if let Some(open) = &self.open_thought {
            return open.index;
        }

        let index = self.next_step_index;

        self.next_step_index += 1;

        frames.push(sse::format_frame(
            Some(super::EVENT_STEP_START),
            &format::render_json(&StartPayload {
                step_index: index,
                step: StartStep::Thought { id: &id },
            }),
        ));

        self.open_thought = Some(OpenThoughtStep {
            index,
            id,
            summary: String::new(),
        });

        index
    }

    /// Mints a step identifier for a freshly opened reasoning block.
    fn minted_thought_id() -> String {
        format!("thought_{}", ids::new_uuid())
    }

    /// Closes the open text step with its complete step, when one is open.
    fn close_text_step(&mut self, frames: &mut Vec<String>) {
        if let Some(open) = self.open_text.take() {
            let step = RenderStep::Typed(super::TypedStep::OutputText { text: open.text });

            frames.push(stop_frame(open.index, &step));
        }
    }

    /// Closes every open step and emits the terminal complete frame exactly once.
    fn complete(&mut self) -> Vec<String> {
        if self.is_complete {
            return Vec::new();
        }

        self.is_complete = true;

        let mut frames = self.ensure_created();

        self.close_text_step(&mut frames);

        if let Some(open) = self.open_thought.take() {
            let step = RenderStep::Typed(super::TypedStep::Thought {
                id: open.id,
                summary: (!open.summary.is_empty()).then_some(open.summary),
                signature: None,
            });

            frames.push(stop_frame(open.index, &step));
        }

        for open in std::mem::take(&mut self.open_calls) {
            let step = RenderStep::Typed(super::TypedStep::FunctionCall {
                id: open.id,
                name: open.name,
                arguments: String::new(),
                signature: None,
            });

            frames.push(stop_frame(open.step_index, &step));
        }

        let usage = self.usage.as_ref().map(|usage| OutUsageRef {
            total_input_tokens: usage.input_tokens,
            total_output_tokens: usage
                .output_tokens
                .saturating_sub(usage.reasoning_tokens.unwrap_or(0)),
            total_thought_tokens: usage.reasoning_tokens,
            total_cached_tokens: usage.cached_tokens.as_ref(),
        });

        frames.push(sse::format_frame(
            Some(super::EVENT_COMPLETE),
            &format::render_json(&CompletePayload {
                status: finish_status(self.finish),
                usage,
            }),
        ));

        frames
    }
}

/// Renders a `step.stop` frame for one complete step: a free function because it borrows no
/// renderer state — the step serializes immediately into the owned frame string.
fn stop_frame(step_index: u64, step: &RenderStep) -> String {
    sse::format_frame(
        Some(super::EVENT_STEP_STOP),
        &format::render_json(&StopPayload { step_index, step }),
    )
}

/// Minimal probe reading a stored thought step's identifier.
#[derive(Debug, Deserialize)]
struct OpaqueIdProbe {
    /// Step type tag.
    #[serde(rename = "type")]
    step_type: String,

    /// Step identifier, when the step carries one.
    #[serde(default)]
    id: Option<String>,
}

/// Returns the identifier a same-family opaque step carries, so a done-only block reopens under
/// the stored id instead of a fresh one; anything else yields no hint.
fn opaque_step_id(opaque: &Value) -> Option<String> {
    serde_json::from_value::<OpaqueIdProbe>(opaque.clone())
        .ok()
        .and_then(|probe| {
            (probe.step_type == super::STEP_THOUGHT)
                .then_some(probe.id)
                .flatten()
        })
}

impl ClientSseRenderer for SseRenderer {
    /// Renders one IR event into zero or more native SSE frames.
    fn render(&mut self, event: &crate::ir::IrEvent) -> Vec<String> {
        use crate::ir::IrEvent;

        match event {
            IrEvent::MessageStart { .. } => self.ensure_created(),

            IrEvent::TextDelta(delta) => {
                let mut frames = self.ensure_created();
                let index = self.open_text_step(&mut frames);

                if let Some(open) = &mut self.open_text {
                    open.text.push_str(delta);
                }

                frames.push(sse::format_frame(
                    Some(super::EVENT_STEP_DELTA),
                    &format::render_json(&DeltaPayload {
                        step_index: index,
                        delta: OutDelta::Text {
                            text: delta.as_str(),
                        },
                    }),
                ));

                frames
            }

            IrEvent::ReasoningDelta(delta) => {
                let mut frames = self.ensure_created();
                let index = self.open_thought_step(&mut frames, Self::minted_thought_id());

                if let Some(open) = &mut self.open_thought {
                    open.summary.push_str(delta);
                }

                frames.push(sse::format_frame(
                    Some(super::EVENT_STEP_DELTA),
                    &format::render_json(&DeltaPayload {
                        step_index: index,
                        delta: OutDelta::ThoughtSummary {
                            summary: delta.as_str(),
                        },
                    }),
                ));

                frames
            }

            // A done carrying text opens its block when no deltas preceded it; an empty-text
            // done only closes the block the deltas already delivered. Either way the stop
            // carries the complete step: the stored value verbatim for same-family blocks,
            // synthesized from text and signature otherwise.
            IrEvent::ReasoningDone {
                text,
                signature,
                opaque,
            } => {
                let mut frames = self.ensure_created();

                match self.open_thought.take() {
                    Some(open) => {
                        let index = open.index;

                        if let Some(signature) = signature {
                            frames.push(sse::format_frame(
                                Some(super::EVENT_STEP_DELTA),
                                &format::render_json(&DeltaPayload {
                                    step_index: index,
                                    delta: OutDelta::ThoughtSignature {
                                        signature: signature.as_str(),
                                    },
                                }),
                            ));
                        }

                        let step = if super::is_family_thought_step(opaque) {
                            RenderStep::Verbatim(opaque.clone())
                        } else {
                            RenderStep::Typed(super::TypedStep::Thought {
                                id: open.id,
                                summary: (!open.summary.is_empty()).then_some(open.summary),
                                signature: signature.clone(),
                            })
                        };

                        frames.push(stop_frame(index, &step));
                    }

                    // A done carrying the full text with no prior deltas reopens under the
                    // stored id when the opaque is a same-family step, else under a minted one,
                    // so the stop always closes the id its start opened with.
                    None if !text.is_empty() => {
                        let id = opaque_step_id(opaque).unwrap_or_else(Self::minted_thought_id);
                        let index = self.open_thought_step(&mut frames, id.clone());

                        frames.push(sse::format_frame(
                            Some(super::EVENT_STEP_DELTA),
                            &format::render_json(&DeltaPayload {
                                step_index: index,
                                delta: OutDelta::ThoughtSummary {
                                    summary: text.as_str(),
                                },
                            }),
                        ));

                        if let Some(signature) = signature {
                            frames.push(sse::format_frame(
                                Some(super::EVENT_STEP_DELTA),
                                &format::render_json(&DeltaPayload {
                                    step_index: index,
                                    delta: OutDelta::ThoughtSignature {
                                        signature: signature.as_str(),
                                    },
                                }),
                            ));
                        }

                        self.open_thought = None;

                        let step = if super::is_family_thought_step(opaque) {
                            RenderStep::Verbatim(opaque.clone())
                        } else {
                            RenderStep::Typed(super::TypedStep::Thought {
                                id,
                                summary: Some(text.clone()),
                                signature: signature.clone(),
                            })
                        };

                        frames.push(stop_frame(index, &step));
                    }

                    // An empty done with no open block closes nothing: no deltas delivered text
                    // and no signature needs carrying.
                    _ => {}
                }

                frames
            }

            IrEvent::ToolCallDelta {
                index,
                call_id,
                name,
                arguments_delta,
            } => {
                let mut frames = self.ensure_created();

                let step_index = match self
                    .open_calls
                    .iter_mut()
                    .find(|open| open.call_index == *index)
                {
                    Some(open) => {
                        if let Some(call_id) = call_id
                            && open.id.is_empty()
                        {
                            open.id = call_id.clone();
                        }

                        if let Some(name) = name
                            && open.name.is_empty()
                        {
                            open.name = name.clone();
                        }

                        open.step_index
                    }

                    None => {
                        let step_index = self.next_step_index;

                        self.next_step_index += 1;

                        frames.push(sse::format_frame(
                            Some(super::EVENT_STEP_START),
                            &format::render_json(&StartPayload {
                                step_index,
                                step: StartStep::FunctionCall {
                                    id: call_id.as_deref().unwrap_or(""),
                                    name: name.as_deref().unwrap_or(""),
                                },
                            }),
                        ));

                        self.open_calls.push(OpenCallStep {
                            call_index: *index,
                            step_index,
                            id: call_id.clone().unwrap_or_default(),
                            name: name.clone().unwrap_or_default(),
                        });

                        step_index
                    }
                };

                // Empty argument keep-alives carry no bytes and render no frame.
                if !arguments_delta.is_empty() {
                    frames.push(sse::format_frame(
                        Some(super::EVENT_STEP_DELTA),
                        &format::render_json(&DeltaPayload {
                            step_index,
                            delta: OutDelta::ToolArguments {
                                arguments: arguments_delta.as_str(),
                            },
                        }),
                    ));
                }

                frames
            }

            // The stop carries the complete call — id, name, full arguments, signature — so the
            // done-only path opens its step here and the streamed path closes the step its
            // deltas opened. Receivers replace accumulated state with the stop's step.
            IrEvent::ToolCallDone {
                index,
                call_id,
                name,
                arguments,
                thought_signature,
            } => {
                let mut frames = self.ensure_created();

                let step_index = match self
                    .open_calls
                    .iter()
                    .position(|open| open.call_index == *index)
                {
                    Some(position) => self.open_calls.remove(position).step_index,

                    None => {
                        let step_index = self.next_step_index;

                        self.next_step_index += 1;

                        frames.push(sse::format_frame(
                            Some(super::EVENT_STEP_START),
                            &format::render_json(&StartPayload {
                                step_index,
                                step: StartStep::FunctionCall {
                                    id: call_id.as_str(),
                                    name: name.as_str(),
                                },
                            }),
                        ));

                        step_index
                    }
                };

                let step = RenderStep::Typed(super::TypedStep::FunctionCall {
                    id: call_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                    signature: thought_signature.clone(),
                });

                frames.push(stop_frame(step_index, &step));

                frames
            }

            IrEvent::Finish(reason) => {
                let mut frames = self.ensure_created();

                self.finish = Some(*reason);

                // The finish closes the open text step: its stop carries the complete text the
                // deltas already delivered. An open reasoning block without its done is closed
                // defensively at completion; conformant streams always pair them.
                self.close_text_step(&mut frames);

                frames
            }

            IrEvent::Usage(usage) => {
                // Usage rides the terminal complete frame; the latest observation wins.
                self.usage = Some(*usage);

                self.ensure_created()
            }

            IrEvent::Done => self.complete(),
        }
    }

    /// Renders a mid-stream error as a terminal data frame carrying the Gemini error envelope,
    /// classified once through the shared API error.
    fn error_frame(&mut self, error: &crate::api::Error) -> String {
        let (_, envelope) = crate::format::error_gemini::render(error);

        sse::format_data_frame(&format::render_json(&envelope))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::IrFinishReason;

    /// Parses one request body given as JSON with an empty route context.
    fn parse(body: Value) -> Result<ParsedRequest, InvalidRequest> {
        parse_request(body.to_string().as_bytes(), &RouteContext::empty())
    }

    /// A full request parses into the documented IR: the model from the body, instructions,
    /// coalesced messages carrying text, a thought step with its verbatim opaque, a tool call
    /// with its signature, and the matching tool output, plus tools, tool choice, sampling, the
    /// reasoning level, the response format, and the streaming flag.
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
                {"type": "output_text", "text": "hello"},
                {"type": "thought", "id": "th_1", "summary": "considering", "signature": "sig-1"},
                {"type": "function_call", "id": "call_1", "name": "search", "arguments": {
                    "q": "hi",
                }, "signature": "sig-call"},
                {"type": "function_result", "name": "search", "call_id": "call_1", "result": [
                    {"type": "text", "text": "found"},
                ]},
            ],
            "tools": [{
                "type": "function",
                "name": "search",
                "description": "Searches.",
                "parameters": {"type": "object"},
            }],
            "tool_choice": {"mode": "any", "allowed_tools": ["search"]},
            "temperature": 0.5,
            "top_p": 0.9,
            "max_output_tokens": 100,
            "thinking_level": "high",
            "response_format": {"type": "json"},
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
        assert_eq!(parsed.ir.temperature, Some(0.5));

        assert_eq!(parsed.ir.messages.len(), 3);

        #[expect(
            clippy::indexing_slicing,
            reason = "the preceding length assertion guarantees three messages"
        )]
        let thought = parsed.ir.messages[1]
            .parts
            .iter()
            .find_map(|part| match part {
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
                "id": "th_1",
                "summary": "considering",
                "signature": "sig-1",
            }),
        );
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

    /// Every disposition-table 400 rejects naming its field: the chaining controls, the numeric
    /// budgets, stop, seed, modalities, the `validated` tool choice, multiple allowed tools, a
    /// choice without tools, built-in tool types, images in tool results, and unknown members —
    /// while `store` and `thinking_summaries` stay accepted and ignored.
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
            ("last_event_id", serde_json::json!("evt_1")),
            ("thinkingBudget", serde_json::json!(100)),
            ("budget_tokens", serde_json::json!(100)),
            ("stop", serde_json::json!(["x"])),
            ("stop_sequences", serde_json::json!(["x"])),
            ("seed", serde_json::json!(7)),
            ("modalities", serde_json::json!(["image"])),
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

        for body in [
            serde_json::json!({
                "model": "m",
                "input": [],
                "tool_choice": "validated",
                "tools": [{"type": "function", "name": "f"}],
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "tool_choice": {"mode": "any", "allowed_tools": ["a", "b"]},
                "tools": [{"type": "function", "name": "a"}, {"type": "function", "name": "b"}],
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "tool_choice": "auto",
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
                     "result": [{"type": "image", "image_url": "https://example.com/i.png"}]},
                ],
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "thinking_level": "ultra",
            }),
            serde_json::json!({
                "model": "m",
                "input": [],
                "unknown_member": true,
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

        // The presentation-only controls parse: `store` and `thinking_summaries` never affect
        // the IR.
        #[expect(
            clippy::expect_used,
            reason = "the ignored-controls body is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "input": [{"type": "user_input", "content": [{"type": "text", "text": "hi"}]}],
            "store": true,
            "thinking_summaries": "auto",
        }))
        .expect("accepted-and-ignored controls must parse");

        assert_eq!(parsed.ir.messages.len(), 1);
    }

    /// The `function_result` shapes agree: a plain string, a text-block array joining with the
    /// blank-line separator, and the tool's own result object flattened to JSON text all carry
    /// the same content class, while a non-object arguments string is rejected.
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
            reason = "the malformed arguments are constructed to be rejected"
        )]
        let error = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "function_call", "id": "c", "name": "f", "arguments": "[1, 2]"},
            ],
        }))
        .expect_err("non-object arguments must be rejected");

        assert_eq!(error.param.as_deref(), Some("arguments"));
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

    /// The aggregated response renders the Interaction resource: thought steps before the text
    /// step before function calls, same-family opaques replayed verbatim, the finish mapped onto
    /// the status vocabulary, and usage split back into output and thought members.
    ///
    /// # Panics
    ///
    /// Panics if the rendered resource diverges from the contract.
    #[test]
    fn renders_the_aggregated_interaction_resource() {
        use crate::ir::{IrAggregatedToolCall, IrReasoningBlock};

        let stored = serde_json::json!({
            "type": "thought",
            "id": "th_1",
            "summary": "stored",
            "signature": "sig-1",
        });

        let aggregation = IrAggregation {
            text: "hello".to_string(),
            reasoning: "storedfresh".to_string(),
            reasoning_blocks: vec![
                IrReasoningBlock {
                    text: "stored".to_string(),
                    signature: Some("sig-1".to_string()),
                    opaque: stored.clone(),
                },
                IrReasoningBlock::text_only("fresh".to_string()),
            ],
            tool_calls: vec![IrAggregatedToolCall {
                call_id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: r#"{"q":"hi"}"#.to_string(),
                thought_signature: Some("sig-call".to_string()),
            }],
            finish: Some(IrFinishReason::ToolCall),
            usage: Some(IrUsage {
                input_tokens: 10,
                output_tokens: 28,
                cached_tokens: Some(3),
                cache_write_tokens: None,
                reasoning_tokens: Some(8),
            }),
        };

        #[expect(
            clippy::expect_used,
            reason = "the rendered resource always serializes"
        )]
        let rendered: Value =
            serde_json::to_value(render_json_response("public-model", &aggregation))
                .expect("the aggregated resource serializes");

        assert_eq!(
            rendered.pointer("/model"),
            Some(&serde_json::json!("public-model"))
        );
        assert_eq!(
            rendered.pointer("/status"),
            Some(&serde_json::json!(STATUS_REQUIRES_ACTION))
        );

        assert_eq!(rendered.pointer("/steps/0"), Some(&stored));
        assert_eq!(
            rendered.pointer("/steps/1/type"),
            Some(&serde_json::json!("thought"))
        );
        assert_eq!(
            rendered.pointer("/steps/1/summary"),
            Some(&serde_json::json!("fresh"))
        );
        assert_eq!(
            rendered.pointer("/steps/2"),
            Some(&serde_json::json!({"type": "output_text", "text": "hello"})),
        );
        assert_eq!(
            rendered.pointer("/steps/3"),
            Some(&serde_json::json!({
                "type": "function_call",
                "id": "call_1",
                "name": "search",
                "arguments": r#"{"q":"hi"}"#,
                "signature": "sig-call",
            })),
        );
        assert!(
            rendered.pointer("/steps/4").is_none(),
            "the resource renders exactly four steps",
        );

        assert_eq!(
            rendered.pointer("/usage"),
            Some(&serde_json::json!({
                "total_input_tokens": 10,
                "total_output_tokens": 20,
                "total_thought_tokens": 8,
                "total_cached_tokens": 3,
            })),
        );
    }

    /// The gateway's own rendered steps parse back verbatim: replaying the aggregated resource's
    /// steps as the next request's input — the multi-turn loop — must parse (the history-replay
    /// carve-out). The replayed thought keeps its opaque, the call keeps its signature.
    ///
    /// # Panics
    ///
    /// Panics if the replay diverges from the rendered content.
    #[test]
    fn rendered_steps_replay_verbatim() {
        use crate::ir::{IrAggregatedToolCall, IrReasoningBlock};

        let stored = serde_json::json!({
            "type": "thought",
            "id": "th_1",
            "summary": "stored",
            "signature": "sig-1",
        });

        let aggregation = IrAggregation {
            text: "hello".to_string(),
            reasoning: "stored".to_string(),
            reasoning_blocks: vec![IrReasoningBlock {
                text: "stored".to_string(),
                signature: Some("sig-1".to_string()),
                opaque: stored.clone(),
            }],
            tool_calls: vec![IrAggregatedToolCall {
                call_id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: r#"{"q":"hi"}"#.to_string(),
                thought_signature: Some("sig-call".to_string()),
            }],
            finish: Some(IrFinishReason::ToolCall),
            usage: None,
        };

        #[expect(
            clippy::expect_used,
            reason = "the rendered resource always serializes"
        )]
        let rendered: Value =
            serde_json::to_value(render_json_response("public-model", &aggregation))
                .expect("the aggregated resource serializes");

        #[expect(clippy::expect_used, reason = "the resource carries a step array")]
        let steps = rendered
            .pointer("/steps")
            .expect("the aggregated resource carries steps")
            .clone();

        #[expect(
            clippy::expect_used,
            reason = "the replayed steps are constructed to parse"
        )]
        let replayed = parse(serde_json::json!({"model": "public-model", "input": steps}))
            .expect("the gateway's own rendered steps must parse back");

        assert_eq!(replayed.ir.messages.len(), 1);

        #[expect(
            clippy::expect_used,
            reason = "the replay carries one assistant message"
        )]
        let message = replayed
            .ir
            .messages
            .first()
            .expect("the replay carries one assistant message");

        assert_eq!(message.role, IrRole::Assistant);
        assert_eq!(message.parts.len(), 3);

        let mut saw_thought = false;
        let mut saw_call = false;
        let mut saw_text = false;
        let mut saw_unexpected = false;

        for part in &message.parts {
            match part {
                IrPart::Reasoning {
                    text,
                    signature,
                    opaque,
                } => {
                    saw_thought = true;
                    assert_eq!(text, "stored");
                    assert_eq!(signature, &Some("sig-1".to_string()));
                    assert_eq!(opaque, &stored);
                }

                IrPart::ToolCall {
                    call_id,
                    name,
                    thought_signature,
                    ..
                } => {
                    saw_call = true;
                    assert_eq!(call_id, "call_1");
                    assert_eq!(name, "search");
                    assert_eq!(thought_signature, &Some("sig-call".to_string()));
                }

                IrPart::Text { text } => {
                    saw_text = true;
                    assert_eq!(text, "hello");
                }

                // Images and tool outputs never appear in a replayed assistant turn.
                IrPart::Image { .. } | IrPart::ToolOutput { .. } => {
                    saw_unexpected = true;
                }
            }
        }

        assert!(
            !saw_unexpected,
            "the replay carries no image or tool-output parts"
        );
        assert!(
            saw_text && saw_thought && saw_call,
            "the replay keeps text, thought, and call"
        );
    }

    /// The streaming renderer speaks the family's event vocabulary end to end: created opens the
    /// stream, text and thought deltas stream under opened steps, an empty-text done closes the
    /// thought with its signature, call deltas assemble and the done closes the call, and the
    /// terminal complete frame carries the mapped status and usage exactly once.
    ///
    /// # Panics
    ///
    /// Panics if the frame sequence diverges from the vocabulary.
    #[test]
    fn renders_the_streaming_vocabulary() {
        use crate::ir::IrEvent;

        let mut renderer = SseRenderer::new("public-model".to_string());

        let mut frames = renderer.render(&IrEvent::MessageStart {
            id: "upstream-id".to_string(),
            model: "upstream-model".to_string(),
        });

        frames.extend(renderer.render(&IrEvent::TextDelta("hi".to_string())));
        frames.extend(renderer.render(&IrEvent::ReasoningDelta("hmm".to_string())));
        frames.extend(renderer.render(&IrEvent::ReasoningDone {
            text: String::new(),
            signature: Some("sig-t".to_string()),
            opaque: serde_json::json!({
                "type": "thought",
                "id": "th_0",
                "summary": "hmm",
                "signature": "sig-t",
            }),
        }));
        frames.extend(renderer.render(&IrEvent::ToolCallDelta {
            index: 0,
            call_id: Some("call_1".to_string()),
            name: Some("search".to_string()),
            arguments_delta: r##"{"q":"##.to_string(),
        }));
        frames.extend(renderer.render(&IrEvent::ToolCallDone {
            index: 0,
            call_id: "call_1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"q":"hi"}"#.to_string(),
            thought_signature: None,
        }));
        frames.extend(renderer.render(&IrEvent::Finish(IrFinishReason::Stop)));
        frames.extend(renderer.render(&IrEvent::Usage(IrUsage {
            input_tokens: 5,
            output_tokens: 6,
            cached_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
        })));
        frames.extend(renderer.render(&IrEvent::Done));

        let events: Vec<&str> = frames
            .iter()
            .filter_map(|frame| {
                frame
                    .strip_prefix("event: ")
                    .and_then(|rest| rest.split('\n').next())
            })
            .collect();

        assert_eq!(
            events,
            vec![
                super::super::EVENT_CREATED,
                super::super::EVENT_STEP_START,
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_START,
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_STOP,
                super::super::EVENT_STEP_START,
                super::super::EVENT_STEP_DELTA,
                super::super::EVENT_STEP_STOP,
                super::super::EVENT_STEP_STOP,
                super::super::EVENT_COMPLETE,
            ],
        );

        #[expect(
            clippy::expect_used,
            reason = "the stream ends with the complete frame"
        )]
        let complete = frames
            .last()
            .expect("the stream ends with the complete frame");

        assert!(complete.contains(STATUS_COMPLETED));
        assert!(complete.contains("total_input_tokens"));
    }

    /// A done carrying the full text with no prior deltas still delivers its block: the renderer
    /// opens the step, streams the text, and closes it — the done-only path mirrors the
    /// delta-then-empty-done path.
    ///
    /// # Panics
    ///
    /// Panics if the done-only block loses its text.
    #[test]
    fn renders_a_done_only_reasoning_block() {
        use crate::ir::IrEvent;

        let mut renderer = SseRenderer::new("public-model".to_string());

        renderer.render(&IrEvent::MessageStart {
            id: String::new(),
            model: String::new(),
        });

        let frames = renderer.render(&IrEvent::ReasoningDone {
            text: "whole".to_string(),
            signature: None,
            opaque: Value::Null,
        });

        let joined = frames.join("");

        assert!(joined.contains("thought_summary"));
        assert!(joined.contains("whole"));
        assert!(joined.contains(super::super::EVENT_STEP_STOP));
    }

    /// The mid-stream error frame carries the Gemini error envelope in a data-only frame.
    ///
    /// # Panics
    ///
    /// Panics if the frame diverges from the Gemini error shape.
    #[test]
    fn renders_error_frames_as_gemini_envelopes() {
        let mut renderer = SseRenderer::new("public-model".to_string());

        let frame = renderer.error_frame(&crate::api::Error::InvalidApiKey);

        assert!(frame.starts_with("data: "));

        #[expect(
            clippy::expect_used,
            reason = "the error frame is constructed to parse"
        )]
        let payload: Value = serde_json::from_str(
            frame
                .strip_prefix("data: ")
                .expect("the error frame is a data frame")
                .trim(),
        )
        .expect("the error frame carries JSON");

        // The official Status shape (google.aip.dev/193): a numeric HTTP `code` beside the
        // canonical google.rpc.Code-style `status` name.
        assert_eq!(
            payload.pointer("/error/code"),
            Some(&serde_json::json!(401))
        );
        assert_eq!(
            payload.pointer("/error/status"),
            Some(&serde_json::json!("UNAUTHENTICATED"))
        );
    }
}
