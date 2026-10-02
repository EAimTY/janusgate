//! OpenAI Chat Completions protocol codec, the Chat Completions family of the OpenAI protocol
//! under [`crate::format::openai`].
//!
//! Serves the outward `POST /chat/completions` endpoint and speaks the `openai_chat_completion`
//! upstream protocol.

use self::client_render::{ChatCompletionResponse, SseRenderer};
use crate::{
    format::{ClientFormat, ClientSseRenderer, ParsedRequest, RouteContext},
    ir::{InvalidRequest, IrAggregation, IrFinishReason},
};

mod client_render;
mod request_parse;
mod upstream_parse;
mod upstream_render;

pub(crate) use self::{upstream_parse::UpstreamParser, upstream_render::render_request_body};

/// Marker of the outward Chat Completions client format.
#[derive(Debug)]
pub(crate) struct Format;

impl ClientFormat for Format {
    type JsonResponse = ChatCompletionResponse;

    fn parse_request(body: &[u8], _route: &RouteContext) -> Result<ParsedRequest, InvalidRequest> {
        request_parse::parse_request(body)
    }

    fn sse_renderer(
        should_include_usage: bool,
        public_id: String,
    ) -> Box<dyn ClientSseRenderer + Send> {
        Box::new(SseRenderer::new(public_id, should_include_usage))
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

/// Maps an OpenAI `finish_reason` string to the canonical reason; unknown values degrade to
/// [`IrFinishReason::Stop`] rather than failing a healthy stream.
fn finish_reason_from_openai(value: &str) -> IrFinishReason {
    match value {
        "length" => IrFinishReason::Length,
        "tool_calls" | "function_call" => IrFinishReason::ToolCall,
        "content_filter" => IrFinishReason::ContentFilter,
        _ => IrFinishReason::Stop,
    }
}

/// Renders the canonical reason as an OpenAI `finish_reason` string.
fn finish_reason_to_openai(reason: IrFinishReason) -> &'static str {
    match reason {
        IrFinishReason::Stop => "stop",
        IrFinishReason::Length => "length",
        IrFinishReason::ToolCall => "tool_calls",
        IrFinishReason::ContentFilter => "content_filter",
    }
}
