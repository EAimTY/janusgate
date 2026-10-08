//! Anthropic Messages codec: the `anthropic_messages` upstream format and the inbound
//! `/api/anthropic-v1/messages` client surface (README §§5.8, 6.2–6.6) — the Messages family of
//! the Anthropic protocol under [`crate::format::anthropic`].
//!
//! The family splits by concern: `request_parse` parses the inbound body, `client_render` renders
//! native Messages completions, `upstream_render` renders the upstream request, and
//! `upstream_parse` parses upstream streams and bodies. Same-family thinking/redacted blocks
//! round-trip byte-exact through the verbatim `opaque` replay payload; cross-family the opaque
//! payload is dropped.
//!
//! The content-block types here are the shared vocabulary of both rendering directions: the
//! upstream request and the client response speak the same block shapes, and the parsers rebuild
//! the same blocks as opaque replay payloads.

use self::client_render::MessagesResponse;
pub(crate) use self::client_render::SseRenderer;
use crate::{
    format::{ClientFormat, ClientSseRenderer, ParsedRequest, RouteContext},
    ir::{InvalidRequest, IrAggregation},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod affinity;
mod client_render;
mod compaction;
mod compaction_affinity;
mod json_affinity;
mod request_parse;
mod upstream_parse;
mod upstream_render;

pub(crate) use self::{
    client_render::{CompactionOnlyUsage, render_compaction_only_response},
    compaction::{GATEWAY_CARRIER_DESCRIPTION, gateway_carrier_block},
    compaction_affinity::{
        checkpoint_projection as completed_compaction_projection,
        completed_anchors as completed_compaction_anchors,
    },
    upstream_parse::UpstreamParser,
    upstream_render::{
        render_count_tokens_request, render_on_demand_body, render_request_body,
        render_threshold_body,
    },
};

/// Marker of the inbound Anthropic Messages client format.
#[derive(Debug)]
pub(crate) struct Format;

impl ClientFormat for Format {
    type JsonResponse = MessagesResponse;

    fn parse_request(body: &[u8], route: &RouteContext) -> Result<ParsedRequest, InvalidRequest> {
        request_parse::parse_inbound_request(body, &route.anthropic_betas)
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

    fn project_json_response(
        response: &Self::JsonResponse,
        projector: &mut crate::affinity::fingerprint::Projector,
    ) -> bool {
        client_render::project_json_response(response, projector)
    }

    fn completed_json_artifacts(response: &Self::JsonResponse) -> Vec<crate::affinity::Anchor> {
        client_render::completed_json_artifacts(response)
    }

    fn project_json_checkpoint(
        response: &Self::JsonResponse,
        projector: &mut crate::affinity::fingerprint::Projector,
    ) -> bool {
        client_render::project_json_after_prefix(response, projector, 1)
    }

    fn render_json_response_with_prefix(
        public_id: &str,
        aggregation: &IrAggregation,
        prefix: &[crate::format::compaction_wire::Checkpoint],
        iterations: &[crate::ir::compaction::UsageIteration],
    ) -> Self::JsonResponse {
        client_render::render_json_response_with_prefix_blocks(
            public_id,
            aggregation,
            prefix,
            iterations,
        )
    }
}

/// One Anthropic content block in the outward direction, covering both the upstream request and
/// the client response shapes, which share the block vocabulary.
///
/// [`OutBlock::Verbatim`] carries a same-family opaque replay payload — an inbound
/// thinking/redacted block stored verbatim — so the gateway replays it without modification;
/// [`OutBlock::Typed`] carries everything the gateway itself constructs.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum OutBlock {
    /// A block the gateway constructed from IR parts.
    Typed(TypedBlock),

    /// A concrete gateway checkpoint or an explicitly opaque native checkpoint.
    Checkpoint(crate::format::compaction_wire::Checkpoint),

    /// A same-family block replayed verbatim from its stored opaque payload: the fragment's
    /// shape is the family's to define, so it travels as a [`Value`].
    Verbatim(Value),
}

/// A content block the gateway constructed.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TypedBlock {
    /// Plain text.
    Text {
        /// The text content.
        text: String,
    },

    /// Image content.
    Image {
        /// Inline data or remote URL source.
        source: ImageSourceOut,
    },

    /// Assistant tool invocation.
    ToolUse {
        /// Identifier of the invocation, round-tripped to the matching result.
        id: String,

        /// Invoked function name.
        name: String,

        /// JSON-encoded arguments object.
        input: Value,
    },

    /// Tool result inside a user turn.
    ToolResult {
        /// Identifier of the invocation being answered.
        tool_use_id: String,

        /// Result content, rendered as a plain string.
        content: String,

        /// Error flag, carried only when the tool reported an error.
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },

    /// Assistant reasoning text with its upstream signature.
    Thinking {
        /// Reasoning text.
        thinking: String,

        /// Upstream signature terminating the block, when the family exposes one.
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// Opaque redacted reasoning the gateway cannot read.
    RedactedThinking {
        /// Redacted payload, verbatim from the family.
        data: Value,
    },
}

/// Image source in the outward direction.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ImageSourceOut {
    /// Base64-encoded inline data.
    Base64 {
        /// MIME type of the image data.
        media_type: String,

        /// Base64-encoded image bytes.
        data: String,
    },

    /// Remote image referenced by URL.
    Url {
        /// Remote `http`/`https` URL.
        url: String,
    },
}

/// Serializes a constructed block into its opaque replay value: under strict inbound parsing the
/// typed struct already captured the block completely, so the value is the block verbatim.
///
/// # Panics
///
/// Panics if the first-party block type fails to serialize, which cannot occur.
fn block_opaque(block: &TypedBlock) -> Value {
    #[expect(
        clippy::expect_used,
        reason = "constructed blocks are first-party types whose serialization is total"
    )]
    serde_json::to_value(block).expect("first-party wire types serialize")
}

/// Probes whether a stored opaque payload is an Anthropic thinking-family block, through a typed
/// probe rather than ad-hoc JSON navigation.
#[derive(Debug, Deserialize)]
struct OpaqueKind {
    /// Block type tag, when the fragment carries one.
    #[serde(default)]
    #[serde(rename = "type")]
    kind: Option<String>,

    /// Upstream signature, when the fragment is a signature-bearing `thinking` block.
    #[serde(default)]
    signature: Option<String>,
}

/// Reports whether the opaque fragment is a same-family thinking/redacted block whose payload
/// replays verbatim; every other fragment — cross-family state or bare text — is not.
///
/// A `thinking` fragment qualifies only with a non-blank `signature`: the official
/// `ThinkingBlockParam` requires the upstream's own signature, so a missing or blank one is not
/// replayable state and both render surfaces degrade it exactly like a foreign-family fragment
/// (README §5.8). A `redacted_thinking` fragment carries `data` and no signature member by
/// design, so it always qualifies.
fn is_anthropic_block(opaque: &Value) -> bool {
    // Deserializing from the borrowed value copies only the tag and signature Strings, never the
    // stored thinking text or redacted blob.
    let Ok(probe) = OpaqueKind::deserialize(opaque) else {
        return false;
    };

    match probe.kind.as_deref() {
        Some("redacted_thinking") => true,
        Some("thinking") => probe
            .signature
            .is_some_and(|signature| !signature.trim().is_empty()),
        _ => false,
    }
}
