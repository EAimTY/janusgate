//! OpenAI Responses protocol codec, the Responses family of the OpenAI protocol under
//! [`crate::format::openai`].
//!
//! Serves the outward `POST /responses` and `POST /responses/compact` endpoints and speaks the
//! `openai_response` upstream protocol (including the Codex backend). The compact endpoint's
//! payloads, state-item builders, and trigger lifecycle renderer live in [`compaction`].

use self::client_render::ResponseObject;
pub(crate) use self::client_render::SseRenderer;
use crate::{
    format::{ClientFormat, ClientSseRenderer, ParsedRequest, RouteContext},
    ir::{InvalidRequest, IrAggregation},
};

mod affinity;
mod client_render;
mod compaction;
mod json_affinity;
mod request_parse;
mod upstream_parse;
mod upstream_render;
mod window_affinity;

pub(crate) use self::{
    compaction::{
        CompactedResponseBody, compaction_item, gateway_window_anchors, parse_compact_request,
        render_compacted_response, render_gateway_compacted_response, render_trigger_stream_frames,
    },
    upstream_parse::{
        CompactedResponseNative, UpstreamParser, UpstreamUsage, parse_compact_response,
    },
    upstream_render::{render_compact_request_body, render_request_body_with_extensions},
};

/// Marker of the outward Responses client format.
#[derive(Debug)]
pub(crate) struct Format;

impl ClientFormat for Format {
    type JsonResponse = ResponseObject;

    fn parse_request(body: &[u8], _route: &RouteContext) -> Result<ParsedRequest, InvalidRequest> {
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

    fn project_json_response(
        response: &Self::JsonResponse,
        projector: &mut crate::affinity::fingerprint::Projector,
    ) -> bool {
        json_affinity::project_json_response(response, projector)
    }

    fn completed_json_artifacts(response: &Self::JsonResponse) -> Vec<crate::affinity::Anchor> {
        json_affinity::completed_json_artifacts(response)
    }

    fn project_json_checkpoint(
        response: &Self::JsonResponse,
        projector: &mut crate::affinity::fingerprint::Projector,
    ) -> bool {
        json_affinity::project_json_after_prefix(response, projector, 1)
    }

    fn render_json_response_with_prefix(
        public_id: &str,
        aggregation: &IrAggregation,
        prefix: &[crate::format::compaction_wire::Checkpoint],
        _iterations: &[crate::ir::compaction::UsageIteration],
    ) -> Self::JsonResponse {
        client_render::render_json_response_with_prefix_items(
            public_id,
            aggregation,
            prefix.to_vec(),
        )
    }
}

/// Borrows one sealer-proven checkpoint window into its canonical active projection.
pub(crate) fn gateway_checkpoint_projection(
    items: &[crate::format::compaction_wire::Checkpoint],
) -> Option<crate::affinity::fingerprint::Projector> {
    window_affinity::project(items, true, true)
}
