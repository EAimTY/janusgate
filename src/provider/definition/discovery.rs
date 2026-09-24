//! The discovery descriptor: the optional upstream model-list request, extraction, and pagination
//! a provider definition configures (README §5.5). An absent discovery means manual models
//! only.
//!
//! Discovery configures a request, a response extraction, and a pagination loop; it discovers
//! upstream model identifiers and never assigns protocol capabilities. A discovered model that
//! does not support its definition's format produces the upstream error when called.

use super::template::{ControlRequest, ResponsePath};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;
/// The model-list extraction of a discovery (README §5.5): the orthogonal fields covering
/// `data[].id`, `items[].slug`, `models[].name`, and map keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryExtract {
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
    },

    /// The identifiers are the keys of one upstream object.
    MapKeys {
        /// The closed path of the object.
        at: ResponsePath,
    },
}

/// The pagination loop of a discovery (README §5.5). Even the no-pagination mode is the tagged
/// `{"type": "none"}` object, never a bare string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Pagination {
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

/// The upstream model-list discovery of one provider definition (README §5.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    /// The control request fetching one model-list page.
    pub request: ControlRequest,

    /// The extraction yielding the page's identifiers.
    pub extract: DiscoveryExtract,

    /// The pagination loop; absent means the list fits one response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<Pagination>,
}
