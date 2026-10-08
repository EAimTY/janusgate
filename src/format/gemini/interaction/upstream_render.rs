//! Upstream-request rendering for the Gemini Interactions codec: renders an IR request into the
//! `gemini_interaction` upstream payload, `POST {base}/v1beta/interactions` carrying the resolved
//! upstream id as the body `model`.

use super::{JSON_MIME_TYPE, ResponseFormatShape};
use crate::{
    format::{self, gemini::is_gemini_thought_step},
    ir::{
        InvalidRequest, IrImage, IrMessage, IrPart, IrRequest, IrResponseFormat, IrRole,
        IrToolChoice,
    },
};
use serde::Serialize;
use serde_json::Value;

/// Renders the upstream request URL and payload: `POST {base}/v1beta/interactions` carrying the
/// resolved `upstream_id` as the body `model`, the stateless full-history `input` replay, and
/// always `store: false`.
///
/// # Errors
///
/// Returns [`InvalidRequest`] when a tool output answers an unknown call id, or when tool call
/// arguments are not a JSON object.
pub(crate) fn render_request_body<'a>(
    ir: &'a IrRequest,
    upstream_id: &'a str,
    is_streaming: bool,
) -> Result<RequestBody<'a>, InvalidRequest> {
    // The Interactions protocol has no compaction state channel: opaque
    // native state another surface produced is never silently discarded, so
    // its presence fails the render (the caller's replay is producer-bound).
    if !ir.state_items.is_empty() {
        return Err(InvalidRequest::request(
            "the request carries compaction state the Interactions upstream cannot replay",
        ));
    }

    let body = RequestBody {
        model: upstream_id,
        system_instruction: ir.instructions.as_deref(),
        input: render_input(ir)?,
        tools: render_tools(ir),
        generation_config: render_generation_config(ir),
        response_format: render_response_format(ir),
        stream: is_streaming,
        // The gateway never stores or chains: every call replays the full history statelessly.
        store: false,
    };

    Ok(body)
}

/// The upstream Interactions request payload.
///
/// The body borrows the request and the resolved upstream id it renders from; ownership is
/// confined to the members derived anew — the parsed function-call argument objects, the resolved
/// tool-output names, and the response-format schema the shared parse/render shape owns.
#[derive(Debug, Serialize)]
pub(crate) struct RequestBody<'a> {
    /// Upstream model name, rendered verbatim from the resolved `upstream_id`.
    model: &'a str,

    /// Concatenated system instructions, omitted when the request has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<&'a str>,

    /// Stateless full-history replay: every turn so far, oldest first.
    input: Vec<RenderStep<'a>>,

    /// Advertised function tools, omitted when the request has none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<UpstreamTool<'a>>,

    /// Reasoning and tool-choice configuration, omitted when it would be empty. The official
    /// `generation_config` pins no sampling members — no `temperature`, no `top_p` — so IR
    /// sampling values have no wire slot on this family and are dropped, never rewritten.
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig<'a>>,

    /// Response format restriction, omitted for free-form text.
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormatShape>,

    /// Whether the upstream streams its response as SSE.
    stream: bool,

    /// Always `false`: the gateway is stateless.
    store: bool,
}

/// One advertised function tool in the official shape: function tools only.
#[derive(Debug, Serialize)]
struct UpstreamTool<'a> {
    /// Tool kind, always `function`.
    #[serde(rename = "type")]
    tool_type: &'static str,

    /// Function name the model calls.
    name: &'a str,

    /// Human-readable description, omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,

    /// JSON Schema of the accepted arguments, omitted for a function that takes none.
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<&'a Value>,
}

/// Reasoning and tool-choice configuration of an upstream request.
#[derive(Debug, Serialize)]
struct GenerationConfig<'a> {
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
    tool_choice: Option<UpstreamToolChoice<'a>>,
}

impl GenerationConfig<'_> {
    /// Whether every field is absent, in which case the whole configuration is omitted from the
    /// request.
    fn is_empty(&self) -> bool {
        self.max_output_tokens.is_none()
            && self.thinking_level.is_none()
            && self.tool_choice.is_none()
    }
}

/// Tool selection restriction in the official shape: a bare mode, or the allowed-tools object
/// forcing one function.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum UpstreamToolChoice<'a> {
    /// The bare mode form.
    Mode(ToolChoiceMode),

    /// The forced-function form: the `any` mode with one allowed tool.
    Allowed {
        /// The allowed-tools configuration.
        allowed_tools: AllowedTools<'a>,
    },
}

/// The allowed-tools configuration of a forced function.
#[derive(Debug, Serialize)]
struct AllowedTools<'a> {
    /// Selection mode, always `any` when forcing one function.
    mode: ToolChoiceMode,

    /// The one function the model may call.
    tools: Vec<&'a str>,
}

/// The official `tool_choice` mode values.
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

/// One rendered input step of the official Interactions vocabulary, borrowing the IR it renders
/// from: either a typed step, or a same-family thought step replayed verbatim from its stored
/// value. The borrowed counterpart of the client render's owned vocabulary — the wire shape is
/// the one the module docs pin, so serializing either spells the same steps.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum RenderStep<'a> {
    /// A same-family thought step replayed verbatim, borrowed from its stored opaque payload.
    Verbatim(&'a Value),

    /// A step rendered from IR content.
    Typed(TypedStep<'a>),
}

/// One typed input step of the official Interactions vocabulary, borrowing the IR it renders
/// from; the function-call arguments are the one owned member, the parsed object the IR stores
/// as JSON text.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum TypedStep<'a> {
    /// A user turn.
    #[serde(rename = "user_input")]
    UserInput {
        /// The turn's content blocks in order.
        content: Vec<ContentBlock<'a>>,
    },

    /// A model output turn.
    #[serde(rename = "model_output")]
    ModelOutput {
        /// The turn's content blocks in order.
        content: Vec<ContentBlock<'a>>,
    },

    /// A model reasoning block synthesized from IR text and signature.
    #[serde(rename = "thought")]
    Thought {
        /// Reasoning summary as the official content-block array, omitted for an empty block.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        summary: Vec<ContentBlock<'a>>,

        /// Upstream signature terminating the block, omitted when absent.
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<&'a str>,
    },

    /// A model tool invocation. The official step carries no signature member: the replayable
    /// thought signature rides the `thought` step.
    #[serde(rename = "function_call")]
    FunctionCall {
        /// Identifier of the invocation, round-tripped to the matching tool output.
        id: &'a str,

        /// Name of the invoked function.
        name: &'a str,

        /// The arguments as the official JSON object; the object's own shape is the tool's schema
        /// to define, so it rides as an opaque JSON object.
        arguments: Value,
    },

    /// A tool result for a prior function call.
    #[serde(rename = "function_result")]
    FunctionResult {
        /// Name of the function being answered, resolved from the conversation.
        name: String,

        /// Identifier of the invocation being answered.
        call_id: &'a str,

        /// Result content as text blocks.
        result: Vec<ContentBlock<'a>>,
    },
}

/// One content block of a `user_input`, `model_output`, or `function_result` step, borrowing the
/// IR text it renders from.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ContentBlock<'a> {
    /// Plain text.
    #[serde(rename = "text")]
    Text {
        /// The text content.
        text: &'a str,
    },

    /// An image in the official spelling: a remote `uri`, or inline base64 `data` with its
    /// `mime_type`. Exactly one spelling is ever constructed.
    #[serde(rename = "image")]
    Image {
        /// Remote image URL, rendered for a URL image.
        #[serde(skip_serializing_if = "Option::is_none")]
        uri: Option<&'a str>,

        /// Base64-encoded image data, rendered for an inline image.
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<&'a str>,

        /// MIME type of the inline data.
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<&'a str>,
    },
}

/// Renders the stateless full-history `input` replay: every message in order, with consecutive
/// user messages coalesced into one `user_input` step whose blocks concatenate, while assistant
/// and tool messages render one step per rendered part, exactly the 1:1 shape.
///
/// Runs are keyed on message role, so any assistant or tool message closes the user run it
/// follows even when none of its own parts render; a user run whose parts all drop renders no
/// step.
///
/// # Errors
///
/// Returns [`InvalidRequest`] per [`format::gemini::resolve_tool_output_name`] and for non-object
/// tool call arguments.
fn render_input(ir: &IrRequest) -> Result<Vec<RenderStep<'_>>, InvalidRequest> {
    // The contribution is the message itself, so runs break exactly where the message role
    // changes; each closed run renders whole below.
    let mut runs: format::RoleRun<IrRole, &IrMessage> = format::RoleRun::default();

    for message in &ir.messages {
        runs.push(message.role, vec![message]);
    }

    let mut steps: Vec<RenderStep<'_>> = Vec::new();

    for (role, messages) in runs.into_runs() {
        match role {
            IrRole::User => {
                let content = messages
                    .iter()
                    .flat_map(|message| &message.parts)
                    .filter_map(|part| match part {
                        IrPart::Text { text } => Some(ContentBlock::Text { text }),

                        IrPart::Image { image } => Some(render_image(image)),

                        // A user turn has no slot for calls, outputs, or reasoning: the IR type
                        // system never routes them here, and a misplaced part is dropped rather
                        // than rendered into a step whose role would misattribute it.
                        IrPart::ToolCall { .. }
                        | IrPart::ToolOutput { .. }
                        | IrPart::Reasoning { .. } => None,
                    })
                    .collect::<Vec<_>>();

                if !content.is_empty() {
                    steps.push(RenderStep::Typed(TypedStep::UserInput { content }));
                }
            }

            IrRole::Assistant => {
                for message in &messages {
                    for part in &message.parts {
                        match part {
                            IrPart::Text { text } => {
                                steps.push(RenderStep::Typed(TypedStep::ModelOutput {
                                    content: vec![ContentBlock::Text { text }],
                                }));
                            }

                            IrPart::Image { .. } | IrPart::ToolOutput { .. } => {
                                // An assistant turn has no image or tool-output slot on this
                                // family's wire: images are request
                                // content and outputs answer calls, so a
                                // misplaced part is dropped rather than rendered into a foreign
                                // step.
                            }

                            IrPart::ToolCall {
                                call_id,
                                name,
                                arguments,
                                // The official function_call step carries no signature member: the
                                // replayable thought signature rides the `thought` step, so this
                                // family's tool-call signature has no wire slot here.
                                thought_signature: _,
                            } => {
                                let arguments = format::arguments_as_object(arguments)?;

                                steps.push(RenderStep::Typed(TypedStep::FunctionCall {
                                    id: call_id,
                                    name,
                                    arguments: Value::Object(arguments),
                                }));
                            }

                            IrPart::Reasoning {
                                text,
                                signature,
                                opaque,
                            } => steps.push(render_thought(text, signature.as_deref(), opaque)),
                        }
                    }
                }
            }

            IrRole::Tool => {
                for message in &messages {
                    for part in &message.parts {
                        if let IrPart::ToolOutput {
                            call_id, content, ..
                        } = part
                        {
                            let name = format::gemini::resolve_tool_output_name(call_id, ir)?;

                            steps.push(RenderStep::Typed(TypedStep::FunctionResult {
                                name,
                                call_id,
                                result: vec![ContentBlock::Text { text: content }],
                            }));
                        }
                    }
                }
            }
        }
    }

    Ok(steps)
}

/// Renders one reasoning block as a thought step: the stored step object replays verbatim when it
/// is a well-formed thought step of this family, else the block synthesizes a fresh step from its
/// text and signature and the opaque payload is dropped — the same rule the client render applies
/// to its owned step vocabulary, keyed on the one shared family predicate.
fn render_thought<'a>(
    text: &'a str,
    signature: Option<&'a str>,
    opaque: &'a Value,
) -> RenderStep<'a> {
    if is_gemini_thought_step(opaque) {
        return RenderStep::Verbatim(opaque);
    }

    let summary = if text.is_empty() {
        Vec::new()
    } else {
        vec![ContentBlock::Text { text }]
    };

    RenderStep::Typed(TypedStep::Thought { summary, signature })
}

/// Renders one image into the official content-block spelling: a remote URL rides `uri`, inline
/// data rides `data` beside its `mime_type`.
fn render_image(image: &IrImage) -> ContentBlock<'_> {
    match image {
        IrImage::Url(url) => ContentBlock::Image {
            uri: Some(url),
            data: None,
            mime_type: None,
        },

        IrImage::Inline {
            mime_type,
            data_base64,
        } => ContentBlock::Image {
            uri: None,
            data: Some(data_base64),
            mime_type: Some(mime_type),
        },
    }
}

/// Renders the advertised function tools in the official shape.
fn render_tools(ir: &IrRequest) -> Vec<UpstreamTool<'_>> {
    ir.tools
        .iter()
        .map(|tool| UpstreamTool {
            tool_type: "function",
            name: &tool.name,
            description: tool.description.as_deref(),
            parameters: tool.parameters.as_ref(),
        })
        .collect()
}

/// Renders the reasoning and tool-choice configuration; `None` when every field is absent, so the
/// whole member is omitted from the request.
fn render_generation_config(ir: &IrRequest) -> Option<GenerationConfig<'_>> {
    let tool_choice = ir
        .tool_choice
        .as_ref()
        .map(|tool_choice| match tool_choice {
            IrToolChoice::Auto => UpstreamToolChoice::Mode(ToolChoiceMode::Auto),

            IrToolChoice::None => UpstreamToolChoice::Mode(ToolChoiceMode::None),

            IrToolChoice::Required => UpstreamToolChoice::Mode(ToolChoiceMode::Any),

            IrToolChoice::Function { name } => UpstreamToolChoice::Allowed {
                allowed_tools: AllowedTools {
                    mode: ToolChoiceMode::Any,
                    tools: vec![name.as_str()],
                },
            },
        });

    let config = GenerationConfig {
        max_output_tokens: ir.max_output_tokens,
        thinking_level: ir.reasoning_effort.as_ref().map(|level| level.as_str()),
        tool_choice,
    };

    (!config.is_empty()).then_some(config)
}

/// Renders the response format restriction into the official `text` member: free-form text renders
/// no member at all, JSON-constrained output rides `mime_type: "application/json"`, and a schema
/// rides beside it — the schema's OpenAI-style name has no official slot here and is dropped.
fn render_response_format(ir: &IrRequest) -> Option<ResponseFormatShape> {
    match &ir.response_format {
        None | Some(IrResponseFormat::Text) => None,

        Some(IrResponseFormat::Json) => Some(ResponseFormatShape::Text {
            mime_type: Some(JSON_MIME_TYPE.to_string()),
            schema: None,
        }),

        Some(IrResponseFormat::JsonSchema { schema, .. }) => Some(ResponseFormatShape::Text {
            mime_type: Some(JSON_MIME_TYPE.to_string()),
            schema: Some(schema.clone()),
        }),
    }
}
