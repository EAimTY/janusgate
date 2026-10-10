//! The discovery descriptor: the optional upstream model-list request, extraction, and pagination
//! a provider definition configures (`docs/design/providers.md`). An absent discovery means manual
//! models only.
//!
//! Discovery configures a request, a response extraction, and a pagination loop; it discovers
//! upstream model identifiers and never assigns protocol capabilities. A discovered model that
//! does not support its definition's format produces the upstream error when called.

use super::template::{ControlRequest, ResponsePath};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;
/// The model-list extraction of a discovery (`docs/design/providers.md`): the orthogonal fields
/// covering `data[].id`, `items[].slug`, `models[].name`, and map keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[allow(
    clippy::large_enum_variant,
    reason = "control-plane extraction descriptors do not allocate per streamed event"
)]
pub(in crate::provider) enum DiscoveryExtract {
    /// The identifiers are the entries of one upstream array.
    Array {
        /// The closed path of the array.
        array_at: ResponsePath,

        /// The closed path of the identifier inside each entry.
        id_field: ResponsePath,

        /// The prefix stripped from every identifier before storage — the `models/<id>` resource
        /// names Gemini lists carry.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        strip_prefix: Option<String>,

        /// The per-entry capability extraction (`docs/design/providers.md`); absent means the
        /// discovery extracts identifiers only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capabilities: Option<CapabilityExtraction>,
    },

    /// The identifiers are the keys of one upstream object. The map-keys shape carries no
    /// per-entry object, so it declares no capability extraction — its models store every
    /// capability unknown.
    MapKeys {
        /// The closed path of the object.
        at: ResponsePath,
    },
}

/// The per-entry capability extraction a discovery may declare (`docs/design/providers.md`): the
/// optional facts each listed model entry carries beside its identifier. Every member is optional —
/// a descriptor that declares none discovers identifiers only, and an entry whose configured
/// path is absent or `null` leaves that capability unknown (`NULL` storage), never a default;
/// a present value of the wrong shape fails the refresh as an upstream error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(in crate::provider) struct CapabilityExtraction {
    /// The thinking-level extraction; absent means levels are never extracted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::provider) thinking_levels: Option<ThinkingLevelsExtraction>,

    /// The maximum-input-tokens member path inside each entry; absent means never extracted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::provider) max_input_tokens: Option<ResponsePath>,

    /// The maximum-output-tokens member path inside each entry; absent means never extracted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::provider) max_output_tokens: Option<ResponsePath>,

    /// The compaction-flag extraction; absent means no compaction fact is ever extracted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::provider) compaction: Option<CompactionCapabilityExtraction>,
}

/// The compaction-flag extraction of a discovery (`docs/design/providers.md`): each configured
/// mechanism reads one boolean flag out of one addressed object of every entry. A `true` flag
/// extracts the mechanism's `supported` fact, a `false` flag its `unsupported` fact, and an
/// absent-or-null object or flag extracts nothing — the mechanism stays unknown. The shape is
/// format-agnostic: the descriptor names upstream paths, and what a mechanism means upstream
/// is the descriptor author's business.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(in crate::provider) struct CompactionCapabilityExtraction {
    /// The Messages on-demand compaction flag (the Anthropic
    /// `capabilities.compaction.supported` shape); absent means never extracted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::provider) messages_on_demand: Option<FlaggedMemberExtraction>,

    /// The Messages threshold compaction flag (the Anthropic
    /// `capabilities.context_management.compact_20260112.supported` shape); absent means never
    /// extracted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::provider) messages_threshold: Option<FlaggedMemberExtraction>,
}

/// One boolean flag read out of one addressed object: the object the `object_at` path names
/// inside an entry, and the boolean its `flag_field` path carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(in crate::provider) struct FlaggedMemberExtraction {
    /// The closed path of the flagged object inside each entry.
    pub(in crate::provider) object_at: ResponsePath,

    /// The closed path of the boolean flag inside the addressed object.
    pub(in crate::provider) flag_field: ResponsePath,
}

/// The two wire shapes a thinking-level set is extracted from (`docs/design/providers.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "shape", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::provider) enum ThinkingLevelsExtraction {
    /// An array of objects, each naming one level in one string member — the Codex backend's
    /// `supported_reasoning_levels[].effort` shape.
    MemberArray {
        /// The closed path of the array.
        array_at: ResponsePath,

        /// The closed path of the level-naming string member inside each element.
        level_field: ResponsePath,
    },

    /// An object whose member names are levels, each member an object carrying its boolean
    /// flag at `flag_field` or a bare boolean standing as its own flag — the Anthropic
    /// `capabilities.effort` shape, whose aggregate `supported` member is a bare boolean no
    /// vocabulary filter can admit.
    FlaggedKeys {
        /// The closed path of the object.
        object_at: ResponsePath,

        /// The closed path of the boolean flag inside each member's value.
        flag_field: ResponsePath,
    },
}

/// The pagination loop of a discovery (`docs/design/providers.md`). Even the no-pagination mode is
/// the tagged `{"type": "none"}` object, never a bare string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::provider) enum Pagination {
    /// The upstream list fits one response.
    None,

    /// Token pagination: each response carries the token of the next page, passed back through
    /// the named request parameter — Gemini's `pageToken`/`nextPageToken`.
    Token {
        /// The request query parameter carrying the next-page token.
        request_param: String,

        /// The closed path of the next-page token in the response.
        response_path: ResponsePath,

        /// The maximum number of pages fetched before the loop gives up.
        max_pages: NonZeroU64,
    },

    /// Cursor pagination: each response states whether more pages follow and the last identifier
    /// seen, passed back through the named request parameter — Anthropic's
    /// `has_more`/`last_id`.
    Cursor {
        /// The closed path of the has-more flag in the response.
        has_more_path: ResponsePath,

        /// The closed path of the last identifier in the response.
        last_id_path: ResponsePath,

        /// The request query parameter carrying the last identifier.
        request_param: String,

        /// The maximum number of pages fetched before the loop gives up.
        max_pages: NonZeroU64,
    },
}

/// The upstream model-list discovery of one provider definition (`docs/design/providers.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(crate) struct Discovery {
    /// The control request fetching one model-list page.
    pub(in crate::provider) request: ControlRequest,

    /// The extraction yielding the page's identifiers.
    pub(in crate::provider) extract: DiscoveryExtract,

    /// The pagination loop; absent means the list fits one response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::provider) pagination: Option<Pagination>,
}
