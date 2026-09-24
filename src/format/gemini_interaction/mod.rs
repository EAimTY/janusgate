//! Gemini Interactions wire codec (`gemini_interaction`).
//!
//! Speaks the `gemini_interaction` upstream protocol — `POST {base}/v1beta/interactions` — and
//! serves the outward `POST /interactions` client surface. Both directions share the one wire
//! vocabulary defined here, and that vocabulary is the official Gemini Interactions API dialect:
//! a provider of this type targets the real endpoint (the `x-goog-api-key` and pinned
//! `Api-Revision` headers, README §5.3), so every member shape below is pinned by the official
//! reference (ai.google.dev/api/interactions-api) and mirrored by the official
//! googleapis/python-genai SDK types. The family splits by concern: `request_parse` parses the
//! inbound body, `client_render` renders the native completion (SSE frames plus the aggregated
//! resource), `upstream_render` renders the upstream request, and `upstream_parse` parses upstream
//! streams and bodies.
//!
//! # Wire contract (normative in README §5.4)
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
//! - `input` is a Step array: `user_input` steps (`content` blocks), model `model_output` steps
//!   (`content` blocks), model `thought` steps (`summary` as a content-block array, `signature`),
//!   model `function_call` steps (`id`, `name`, `arguments` as the JSON object itself — the step
//!   carries no signature member; the replayable thought signature rides the `thought` step), and
//!   client `function_result` steps (`call_id`, optional `name`, `is_error`, `result` blocks).
//!   Assistant text parts that have no other slot render as `model_output` steps.
//! - Streaming upstreams speak SSE frames self-described two ways: by the `event:` line when the
//!   stream carries one, else by the payload's official `event_type` member. The events are
//!   `interaction.created` and the terminal `interaction.completed`, each carrying a partial
//!   Interaction resource under an `interaction` member; `step.start` (`index`, `step`),
//!   `step.delta` (`index`, `delta`), and `step.stop` (`index`, plus optional `step_usage`/`usage`
//!   members the gateway drops — per-step usage would double-count the interaction total that rides
//!   `interaction.completed`); and the progress-only `interaction.status_update`, parsed and
//!   ignored: the terminal status and usage arrive only on `interaction.completed`. Delta kinds:
//!   `text`, `thought_summary` (a content block), `thought_signature`, `arguments_delta`; every
//!   other kind — media output, `text_annotation_delta`, server-side tool traffic, and future kinds
//!   — is parsed and ignored, mirroring how the family treats ignorable steps: a delta kind the IR
//!   cannot represent never aborts a stream of official events.
//! - Non-streaming upstreams answer the Interaction resource (`id`, `model`, `status`, `steps`,
//!   `usage`).
//! - Status maps `requires_action` to tool-call finish, `incomplete` to length, `cancelled` to a
//!   clean stop (deliberate: the interaction ended without failure and the canonical finish
//!   vocabulary has no cancelled member), `failed` to an upstream failure, and the deprecated
//!   `budget_exceeded` to length (the official documents it returning the same result as
//!   `incomplete`). The non-terminal `in_progress`/`queued` statuses, `completed`, and any future
//!   status degrade to a clean stop.
//! - Usage maps `total_input_tokens` to input, `total_output_tokens + total_thought_tokens` to
//!   output (IR output includes reasoning, as in the GenerateContent codec), `total_thought_tokens`
//!   to reasoning, `total_cached_tokens` to cached; there is no cache-write accounting and
//!   `total_tool_use_tokens` is dropped.
//! - The official `generation_config` pins no sampling members — no `temperature`, no `top_p` — so
//!   the gateway renders none and IR sampling values have no wire slot on this family; they are
//!   dropped, never rewritten. The official `response_format` union is `text`/`audio`/`image`/
//!   `video`: the gateway renders the `text` member, with `mime_type: "application/json"` (and the
//!   schema beside it) for JSON-constrained outputs; the schema's optional OpenAI-style name has no
//!   official slot here and is dropped, while the media members are rejected inbound as
//!   known-unsupported.
//! - Same-family thought round-trip is verbatim: a reasoning block whose opaque payload is a
//!   well-formed thought step of this family (an object whose `type` is `thought`) replays
//!   byte-for-byte — members and values preserved, key order canonicalized by `serde_json`'s map
//!   ordering — while any other opaque (cross-family blocks, text-only blocks) renders from
//!   `text`/`signature` and drops the payload. The family-membership test is structural, so a
//!   foreign block can never leak onto this family's wire.
//!
//! # What remains self-pinned
//!
//! The official documents pin the member shapes but not the SSE transport details: whether frames
//! carry an `event:` line at all, and how partial a `step.start`'s `step` object may be. This codec
//! therefore emits both discriminators (the `event:` line and the `event_type` member), accepts
//! either discriminator inbound, parses `step.start` steps tolerantly (kind, `id`, and `name` all
//! defaulting), and emits a thought step's complete JSON only in the aggregated resource — never on
//! the streamed wire, where the official `step.stop` carries only the index.

use self::client_render::{JsonResponse, SseRenderer};
use crate::{
    format::{ClientFormat, ClientSseRenderer, ParsedRequest, RouteContext},
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

/// SSE event opening an upstream stream, carrying the interaction identity under the official
/// `interaction` member.
const EVENT_CREATED: &str = "interaction.created";

/// SSE event terminating an upstream stream, carrying the terminal partial Interaction resource
/// under the official `interaction` member.
const EVENT_COMPLETED: &str = "interaction.completed";

/// SSE progress event carrying an interaction status change; content-free by contract — the
/// terminal status and usage arrive only on [`EVENT_COMPLETED`] — so it is parsed and ignored.
const EVENT_STATUS_UPDATE: &str = "interaction.status_update";

/// SSE event opening one step of an upstream stream, carrying the step index and its opening shape.
const EVENT_STEP_START: &str = "step.start";

/// SSE event carrying one incremental delta of an open step.
const EVENT_STEP_DELTA: &str = "step.delta";

/// SSE event closing one step of an upstream stream; the official payload carries only the index
/// (beside optional usage members the gateway drops), so the streamed deltas are the sole content
/// source.
const EVENT_STEP_STOP: &str = "step.stop";

/// Terminal status of a completed interaction.
const STATUS_COMPLETED: &str = "completed";

/// Terminal status of an interaction awaiting tool execution.
const STATUS_REQUIRES_ACTION: &str = "requires_action";

/// Terminal status of an interaction cut off before completion.
const STATUS_INCOMPLETE: &str = "incomplete";

/// Terminal status of an interaction the caller cancelled; maps to a clean stop.
const STATUS_CANCELLED: &str = "cancelled";

/// Terminal status of a failed interaction; surfaces as an upstream failure, never as a finish.
const STATUS_FAILED: &str = "failed";

/// Deprecated terminal status for token-budget exhaustion; the official documents it returning
/// the same result as `incomplete`, so it maps to the length finish too.
const STATUS_BUDGET_EXCEEDED: &str = "budget_exceeded";

/// Non-streaming status of an interaction still generating; never terminates a stream.
const STATUS_IN_PROGRESS: &str = "in_progress";

/// Step type tag of a model output turn.
const STEP_MODEL_OUTPUT: &str = "model_output";

/// Step type tag of a model reasoning block.
const STEP_THOUGHT: &str = "thought";

/// Step type tag of a model tool invocation.
const STEP_FUNCTION_CALL: &str = "function_call";

/// The `mime_type` value of the official text response format constrained to JSON.
const JSON_MIME_TYPE: &str = "application/json";

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

/// One typed input step of the official Interactions vocabulary.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum TypedStep {
    /// A user turn.
    #[serde(rename = "user_input")]
    UserInput {
        /// The turn's content blocks in order.
        content: Vec<ContentBlock>,
    },

    /// A model output turn.
    #[serde(rename = "model_output")]
    ModelOutput {
        /// The turn's content blocks in order.
        content: Vec<ContentBlock>,
    },

    /// A model reasoning block synthesized from IR text and signature.
    #[serde(rename = "thought")]
    Thought {
        /// Reasoning summary as the official content-block array, omitted for an empty block.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        summary: Vec<ContentBlock>,

        /// Upstream signature terminating the block, omitted when absent.
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// A model tool invocation. The official step carries no signature member: the replayable
    /// thought signature rides the `thought` step.
    #[serde(rename = "function_call")]
    FunctionCall {
        /// Identifier of the invocation, round-tripped to the matching tool output.
        id: String,

        /// Name of the invoked function.
        name: String,

        /// The arguments as the official JSON object; the object's own shape is the tool's schema
        /// to define, so it rides as an opaque JSON object.
        arguments: Value,
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

/// One content block of a `user_input`, `model_output`, or `function_result` step.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ContentBlock {
    /// Plain text.
    #[serde(rename = "text")]
    Text {
        /// The text content.
        text: String,
    },

    /// An image in the official spelling: a remote `uri`, or inline base64 `data` with its
    /// `mime_type`. Exactly one spelling is ever constructed.
    #[serde(rename = "image")]
    Image {
        /// Remote image URL, rendered for a URL image.
        #[serde(skip_serializing_if = "Option::is_none")]
        uri: Option<String>,

        /// Base64-encoded image data, rendered for an inline image.
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<String>,

        /// MIME type of the inline data.
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
}

/// Response format restriction, shared by the upstream renderer and the inbound parser: the two
/// directions speak the same official union member. The union's `audio`/`image`/`video` members
/// are modelled so an inbound one rejects naming the field; the gateway renders the `text` member
/// alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::format) enum ResponseFormatShape {
    /// Text output, optionally constrained to JSON.
    Text {
        /// Output MIME type: `text/plain` (the upstream default) or `application/json`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,

        /// The JSON Schema the output conforms to; only meaningful with `application/json`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<Value>,
    },

    /// Audio output; known-unsupported, rejected inbound naming the field.
    Audio {},

    /// Image output; known-unsupported, rejected inbound naming the field.
    Image {},

    /// Video output; known-unsupported, rejected inbound naming the field.
    Video {},
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

    let summary = if text.is_empty() {
        Vec::new()
    } else {
        vec![ContentBlock::Text {
            text: text.to_string(),
        }]
    };

    RenderStep::Typed(TypedStep::Thought { summary, signature })
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
