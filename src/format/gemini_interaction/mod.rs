//! Gemini Interactions wire codec (`gemini_interaction`).
//!
//! Speaks the `gemini_interaction` upstream protocol — `POST {base}/v1beta/interactions` — and
//! serves the outward `POST /interactions` client surface. Both directions share the one wire
//! vocabulary defined here, so the family is self-consistent by construction. The family splits
//! by concern: `request_parse` parses the inbound body, `client_render` renders the native
//! completion (SSE frames plus the aggregated resource), `upstream_render` renders the upstream
//! request, and `upstream_parse` parses upstream streams and bodies.
//!
//! # Wire contract (this module's design choice, normative in README §5.4)
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

use self::client_render::{JsonResponse, SseRenderer};
use crate::{
    format::{ClientFormat, ClientSseRenderer, ParsedRequest, RouteContext},
    ids,
    ir::{InvalidRequest, IrAggregation},
    storage::api_logs::ApiLogEndpointType,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod client_render;
mod request_parse;
mod upstream_parse;
mod upstream_render;

pub(crate) use self::{upstream_parse::UpstreamParser, upstream_render::render_request};

/// Marker of the inbound Gemini Interactions client format.
#[derive(Debug)]
pub(crate) struct Format;

impl ClientFormat for Format {
    type JsonResponse = JsonResponse;

    const ENDPOINT_TYPE: ApiLogEndpointType = ApiLogEndpointType::GeminiInteractions;

    fn parse_request(body: &[u8], route: &RouteContext) -> Result<ParsedRequest, InvalidRequest> {
        request_parse::parse_request(body, route)
    }

    fn sse_renderer(
        _should_include_usage: bool,
        public_id: String,
    ) -> Box<dyn ClientSseRenderer + Send> {
        Box::new(SseRenderer::new(public_id))
    }

    fn render_json_response(public_id: &str, aggregation: &IrAggregation) -> Self::JsonResponse {
        client_render::render_json_response(public_id, aggregation)
    }
}

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
