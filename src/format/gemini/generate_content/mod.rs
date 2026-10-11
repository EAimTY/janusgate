//! Gemini GenerateContent protocol codec, the GenerateContent family of the Gemini protocol under
//! [`crate::format::gemini`].
//!
//! Speaks the `gemini_generate_content` upstream protocol, both in the Antigravity envelope
//! (`docs/design/providers.md`) and as a bare OpenAI-compatible upstream protocol
//! (`docs/design/providers.md`), and serves the inbound Gemini GenerateContent surface — `POST
//! /models/{model}:generateContent` (non-streaming) and `POST /models/{model}:
//! streamGenerateContent?alt=sse` (streaming, only the `alt=sse` form is supported, else 400) plus
//! `GET /models` — per `docs/reference/inference.md`–6.6.
//!
//! The architecture this surface plugs into:
//! - [`crate::format::RouteContext`] carries the URL-routed public model id and the streaming mode
//!   from the URL action; the body carries no model.
//! - [`crate::vocabulary::ReasoningLevel`] for the inbound level parameter (unknown strings are a
//!   400);
//! - [`crate::ir::IrPart::Reasoning`] never originates here (GenerateContent has no reasoning input
//!   slot); history-replay carve-outs stay minimal — only what the gateway itself emits
//!   (`thoughtSignature` on `functionCall` parts, gateway-rendered `id` fields, and the `thought`
//!   flag accepted-and-ignored as plain text);
//! - [`crate::api::Error`] with [`crate::api::error::gemini`] for both error paths.
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

mod affinity;
mod client_render;
mod json_affinity;
mod request_parse;
mod upstream_parse;
mod upstream_render;

#[cfg(test)]
mod tests;

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

    fn project_json_response(
        response: &Self::JsonResponse,
        projector: &mut crate::affinity::fingerprint::Projector,
    ) -> bool {
        json_affinity::project_json_response(response, projector)
    }
}
