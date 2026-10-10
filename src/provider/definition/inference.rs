//! The inference descriptor: how a provider definition renders and frames upstream inference
//! requests (`docs/design/providers.md`).
//!
//! The format codecs render and parse bodies only; the URL, the headers, the streaming decision,
//! and the response unwrapping are this descriptor's data, applied by the generic dispatcher.

use super::template::{ParamRow, StringExpr, TemplateNode};
use crate::format::RenderOption;
use serde::{Deserialize, Serialize};
/// The upstream streaming mode a provider definition pins (`docs/design/providers.md`): it drives
/// both the rendered body's stream flag and the SSE-versus-JSON response parser choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum UpstreamStreaming {
    /// The upstream always streams: every dispatch requests streaming upstream, and a
    /// non-streaming client receives an aggregated response.
    Always,

    /// The upstream matches the client's requested mode.
    MatchClient,
}

/// The inference request descriptor of one provider definition (`docs/design/providers.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(crate) struct Inference {
    /// The evaluated inference request URL; required.
    pub(crate) url: StringExpr,

    /// The evaluated streaming request URL, used only when the streaming action targets a
    /// different URL; when absent, streaming uses [`Inference::url`]. Stream-in-body formats
    /// normally omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stream_url: Option<StringExpr>,

    /// The evaluated request headers, rendered in the carried order. This list subsumes the auth
    /// style: `Authorization: Bearer <secret>`, `x-api-key: <secret>`, and `x-goog-api-key:
    /// <secret>` are rows here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) headers: Vec<ParamRow>,

    /// The upstream streaming mode.
    pub(crate) upstream_streaming: UpstreamStreaming,

    /// The render options the codec applies to the body; every option must belong to the
    /// definition format's closed set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) render_options: Vec<RenderOption>,

    /// The optional inference request body envelope (`docs/design/providers.md`): a typed JSON
    /// tree whose [`TemplateNode::Body`] — appearing exactly once — is the codec-rendered
    /// upstream body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) envelope: Option<TemplateNode>,

    /// Optional ordered exact quota rejection classifier for generation/native compaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) quota_exhaustion: Option<super::quota_exhaustion::QuotaExhaustion>,

    /// The optional object-member name the codec's parser unwraps before ingesting a response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) response_unwrap: Option<String>,
}
