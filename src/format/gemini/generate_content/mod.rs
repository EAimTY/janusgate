//! Gemini GenerateContent protocol codec, the GenerateContent family of the Gemini protocol under
//! [`crate::format::gemini`].
//!
//! Speaks the `gemini_generate_content` upstream protocol, both in the Antigravity envelope (§5.2)
//! and as a bare OpenAI-compatible upstream protocol (§5.3), and serves the inbound Gemini
//! GenerateContent surface — `POST /models/{model}:generateContent` (non-streaming) and
//! `POST /models/{model}:streamGenerateContent?alt=sse` (streaming, only the `alt=sse` form is
//! supported, else 400) plus `GET /models` — per README §§6.3–6.6.
//!
//! The architecture this surface plugs into:
//! - [`crate::format::RouteContext`] carries the URL-routed public model id and the streaming mode
//!   from the URL action; the body carries no model.
//! - [`crate::ir::IrReasoningLevel`] for the inbound level parameter (unknown strings are a 400);
//! - [`crate::ir::IrPart::Reasoning`] never originates here (GenerateContent has no reasoning input
//!   slot); history-replay carve-outs stay minimal — only what the gateway itself emits
//!   (`thoughtSignature` on `functionCall` parts, gateway-rendered `id` fields, and the `thought`
//!   flag accepted-and-ignored as plain text);
//! - [`crate::api::Error`] with [`crate::format::gemini::error`] for both error paths.
//!
//! Rendered payloads are first-party types in their wire shapes, so field presence per request is
//! declared by `skip_serializing_if` rather than assembled conditionally. Wire shapes follow the
//! official Gemini GenerateContent REST shapes in their camelCase spellings (`systemInstruction`,
//! `generationConfig`, `toolConfig`, `functionCall`, `functionResponse`, `inlineData`, `fileData`,
//! `thoughtSignature`); every inbound field additionally accepts its snake_case alias
//! (`system_instruction`, `generation_config`, …) so callers using either convention parse
//! identically, and truly unknown fields remain 400s through `deny_unknown_fields`.

use self::client_render::{JsonResponse, SseRenderer};
use crate::{
    format::{ClientFormat, ClientSseRenderer, ParsedRequest, RouteContext},
    ir::{InvalidRequest, IrAggregation},
};

mod client_render;
mod request_parse;
mod upstream_parse;
mod upstream_render;

pub(crate) use self::{
    upstream_parse::UpstreamParser, upstream_render::render_generate_content_request,
};

/// Marker of the inbound Gemini GenerateContent client format.
#[derive(Debug)]
pub(crate) struct Format;

impl ClientFormat for Format {
    type JsonResponse = JsonResponse;

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

    fn render_json_response_with_prefix(
        _public_id: &str,
        aggregation: &IrAggregation,
        _prefix: &[serde_json::Value],
        _iterations: &[serde_json::Value],
    ) -> Self::JsonResponse {
        // This surface has no compaction channel; its callers can never
        // produce a checkpoint prefix, so the ordinary rendering stands.
        Self::render_json_response(_public_id, aggregation)
    }
}
