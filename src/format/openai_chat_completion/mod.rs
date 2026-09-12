//! OpenAI Chat Completions protocol codec.
//!
//! Serves the outward `POST /chat/completions` endpoint and speaks the
//! `openai_chat_completion` upstream protocol.

use crate::{
    format::{ClientFormat, ClientSseRenderer, ParsedRequest},
    ir::{InvalidRequest, IrAggregation},
};
use serde_json::Value;

mod client_render;
mod request_parse;
mod upstream_parse;
mod upstream_render;

pub(crate) use self::{
    client_render::{SseRenderer, render_json_response},
    upstream_parse::UpstreamParser,
    upstream_render::render_request,
};

/// Marker of the outward Chat Completions client format.
#[derive(Debug)]
pub(crate) struct Format;

impl ClientFormat for Format {
    fn parse_request(body: &[u8]) -> Result<ParsedRequest, InvalidRequest> {
        request_parse::parse_request(body)
    }

    fn sse_renderer(
        should_include_usage: bool,
        public_name: String,
    ) -> Box<dyn ClientSseRenderer + Send> {
        Box::new(SseRenderer::new(public_name, should_include_usage))
    }

    fn render_json_response(public_name: &str, aggregation: &IrAggregation) -> Value {
        render_json_response(public_name, aggregation)
    }
}
