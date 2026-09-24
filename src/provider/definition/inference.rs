//! The inference descriptor: how a provider definition renders and frames upstream inference
//! requests (README §5.2).
//!
//! The format codecs render and parse bodies only; the URL, the headers, the streaming decision,
//! and the response unwrapping are this descriptor's data, applied by the generic dispatcher.

use super::template::{ParamRow, StringExpr, TemplateNode};
use serde::{Deserialize, Serialize};
/// The upstream streaming mode a provider definition pins (README §5.2): it drives both the
/// rendered body's stream flag and the SSE-versus-JSON response parser choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum UpstreamStreaming {
    /// The upstream always streams: every dispatch requests streaming upstream, and a
    /// non-streaming client receives an aggregated response.
    Always,

    /// The upstream matches the client's requested mode.
    MatchClient,
}

/// One render option of a format codec (README §5.2): a closed, per-format switch adjusting the
/// body rendering. Growing this enum is the documented extension path for a future provider that
/// does not fit the plain body rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RenderOption {
    /// Never render `max_output_tokens`: the upstream rejects the parameter as unsupported, and
    /// the model's own output bound applies instead of the client-requested cap (`openai_response`
    /// only).
    OmitMaxOutputTokens,
}

impl RenderOption {
    /// The canonical option name, rendered by rejections and shared with the serde wire name.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::OmitMaxOutputTokens => "omit_max_output_tokens",
        }
    }
}

/// The inference request descriptor of one provider definition (README §5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Inference {
    /// The evaluated inference request URL; required.
    pub url: StringExpr,

    /// The evaluated streaming request URL, used only when the streaming action targets a
    /// different URL; when absent, streaming uses [`Inference::url`]. Stream-in-body formats
    /// normally omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_url: Option<StringExpr>,

    /// The evaluated request headers, rendered in the carried order. This list subsumes the auth
    /// style: `Authorization: Bearer <secret>`, `x-api-key: <secret>`, and `x-goog-api-key:
    /// <secret>` are rows here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<ParamRow>,

    /// The upstream streaming mode.
    pub upstream_streaming: UpstreamStreaming,

    /// The render options the codec applies to the body; every option must belong to the
    /// definition format's closed set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub render_options: Vec<RenderOption>,

    /// The optional inference request body envelope (README §5.1): a typed JSON tree whose
    /// [`TemplateNode::Body`] — appearing exactly once — is the codec-rendered upstream body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<TemplateNode>,

    /// The optional object-member name the codec's parser unwraps before ingesting a response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_unwrap: Option<String>,
}
