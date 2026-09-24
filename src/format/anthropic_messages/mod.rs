//! Anthropic Messages codec: the `anthropic_messages` upstream format and the inbound
//! `/api/anthropic-v1/messages` client surface (README §§5.8, 6.2–6.6).
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

use self::client_render::{MessagesResponse, SseRenderer};
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

pub(crate) use self::{upstream_parse::UpstreamParser, upstream_render::render_request_body};

/// Marker of the inbound Anthropic Messages client format.
#[derive(Debug)]
pub(crate) struct Format;

impl ClientFormat for Format {
    type JsonResponse = MessagesResponse;

    const ENDPOINT_TYPE: ApiLogEndpointType = ApiLogEndpointType::AnthropicMessages;

    fn parse_request(body: &[u8], _route: &RouteContext) -> Result<ParsedRequest, InvalidRequest> {
        request_parse::parse_inbound_request(body)
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
}

/// Reports whether the opaque fragment is a same-family thinking/redacted block whose payload
/// replays verbatim; every other fragment — cross-family state or bare text — is not.
fn is_anthropic_block(opaque: &Value) -> bool {
    let Ok(probe) = serde_json::from_value::<OpaqueKind>(opaque.clone()) else {
        return false;
    };

    matches!(
        probe.kind.as_deref(),
        Some("thinking") | Some("redacted_thinking")
    )
}
