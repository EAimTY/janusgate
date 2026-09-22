//! OpenAI Responses protocol codec.
//!
//! Serves the outward `POST /responses` endpoint and speaks the `openai_response` upstream protocol
//! (including the Codex backend).

use self::client_render::{ResponseObject, SseRenderer};
use crate::{
    api_logs::ApiLogEndpointType,
    format::{ClientFormat, ClientSseRenderer, ParsedRequest},
    ir::{InvalidRequest, IrAggregation},
};

mod client_render;
mod request_parse;
mod upstream_parse;
mod upstream_render;

pub(crate) use self::{
    upstream_parse::UpstreamParser,
    upstream_render::{render_codex_request, render_request},
};

/// Marker of the outward Responses client format.
#[derive(Debug)]
pub(crate) struct Format;

impl ClientFormat for Format {
    type JsonResponse = ResponseObject;

    const ENDPOINT_TYPE: ApiLogEndpointType = ApiLogEndpointType::OpenaiResponses;

    fn parse_request(body: &[u8]) -> Result<ParsedRequest, InvalidRequest> {
        request_parse::parse_request(body)
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
