//! Product-independent quota descriptor vocabulary (README §5.10).
//!
//! A [`QuotaDescriptor`] is one read-only control request plus an ordered list of extraction
//! sources. Extraction is atomic and strict: absent or null optional sources and entries are
//! omitted, missing optional facts become explicit unknown variants, and a malformed present
//! value fails the complete reading. The three path contexts every [`QuotaField`] selects from
//! are the response `root`, the `item` a source selected, and the `entry` object an entry's own
//! `at` member addresses; the `entry` context is unavailable while evaluating source scope or
//! array identity, where the entry object does not exist yet.

use super::template::{ControlRequest, ResponsePath};
use crate::provider::quota::SafeDuration;
use serde::{Deserialize, Serialize};
use vec1::Vec1;

/// One read-only quota query and its ordered extraction sources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct QuotaDescriptor {
    /// The authenticated control request.
    pub request: ControlRequest,
    /// Nonempty ordered sources; the carrying `Vec1` type rejects an empty list at parse.
    /// Result order follows the declared source order; an empty collection inside an otherwise
    /// well-formed response extracts no observations, while a present malformed value fails the
    /// whole reading.
    #[cfg_attr(
        feature = "management-api-typescript-bindings",
        ts(as = "Vec<QuotaSource>")
    )]
    pub sources: Vec1<QuotaSource>,
}

/// One independently identified allowance within a source item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct QuotaEntryExtraction {
    /// Stable entry identity segment, unique among the source's entries and carried verbatim
    /// into the restriction key tuple.
    pub key: String,
    /// Human-readable restriction name; presentation only — it never selects models or implies
    /// account-wide applicability.
    pub label: String,
    /// Object path relative to the selected item; the object it addresses becomes the entry
    /// context for the entry's own field selections.
    pub at: ResponsePath,
    /// Whether an absent entry fails extraction; an absent optional entry is omitted.
    pub is_required: bool,
    /// Allowance period mapping.
    pub window: QuotaWindowSource,
    /// Active measurement mapping.
    pub measurement: QuotaMeasurementSource,
    /// Reset metadata mapping.
    pub reset: QuotaResetSource,
    /// Authored dispatch applicability, independent of presentation scope and labels.
    #[serde(default)]
    pub routing: QuotaRoutingSource,
    /// Optional explicit boolean that bypasses numeric measurement extraction: a present `true`
    /// yields the unlimited measurement without reading the numeric fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
    pub is_unlimited: Option<QuotaField>,
}

/// Authored dispatch applicability of one independently identified restriction.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaRoutingSource {
    /// Informational only; never blocks dispatch.
    #[default]
    MonitoringOnly,
    /// Applies to every exact physical model on the account.
    AllModels,
    /// Applies only to the nonempty set of exact IDs resolved in this entry's contexts; the
    /// carrying `Vec1` type rejects an empty set at parse.
    Models {
        /// Required text sources; unresolved values invalidate the complete observation.
        #[cfg_attr(
            feature = "management-api-typescript-bindings",
            ts(as = "Vec<QuotaTextSource>")
        )]
        upstream_ids: Vec1<QuotaTextSource>,
    },
}

/// A scalar selection with an explicit response context: the path resolves from the declared
/// [`QuotaFieldBase`] context, and the empty path selects the context object itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct QuotaField {
    /// The context from which the path resolves.
    pub base: QuotaFieldBase,
    /// Object-member chain; empty selects the context itself.
    pub path: ResponsePath,
    /// Whether absence fails extraction; absence of an optional field yields the unknown fact.
    pub is_required: bool,
}

/// A numeric field with an explicit wire encoding: the encoding names the accepted JSON
/// representation, and a present value in any other representation fails the whole reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct QuotaNumberField {
    /// The scalar selection.
    pub field: QuotaField,
    /// Accepted numeric representation.
    pub encoding: QuotaNumberEncoding,
}

/// One source collection; its order and identity determine result order. Every source carries a
/// stable `key` — unique among the descriptor's sources and the first segment of every identity
/// tuple its items produce — and the root-relative `at` path selecting the collection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaSource {
    /// A single whole object: the value `at` addresses is the one item, carrying no identity
    /// segment beyond the source key. A non-object value fails the reading.
    Object {
        /// Stable source identity segment, unique among the descriptor's sources.
        key: String,
        /// Root-relative object-member path selecting the item.
        at: ResponsePath,
        /// Whether an absent source fails extraction; an absent optional source is omitted.
        is_required: bool,
        /// Optional named scope resolved once against the root and the single item; absent is
        /// the unknown scope.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
        scope: Option<QuotaTextSource>,
        /// Nonempty ordered entry extractions applied to the item; the carrying `Vec1` type
        /// rejects an empty list at parse.
        #[cfg_attr(
            feature = "management-api-typescript-bindings",
            ts(as = "Vec<QuotaEntryExtraction>")
        )]
        entries: Vec1<QuotaEntryExtraction>,
    },
    /// An array whose elements are the items, in declared upstream order. Every item must be an
    /// object carrying the required identity fields.
    Array {
        /// Stable source identity segment, unique among the descriptor's sources.
        key: String,
        /// Root-relative object-member path selecting the array.
        at: ResponsePath,
        /// Whether an absent source fails extraction; an absent optional source is omitted.
        is_required: bool,
        /// Nonempty ordered identity text sources evaluated per item against the root and the
        /// item; at least one must be a required item-base field, and `map_key` is unavailable.
        /// The carrying `Vec1` type rejects an empty list at parse.
        #[cfg_attr(
            feature = "management-api-typescript-bindings",
            ts(as = "Vec<QuotaTextSource>")
        )]
        identity: Vec1<QuotaTextSource>,
        /// Optional named scope resolved per item; absent is the unknown scope.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
        scope: Option<QuotaTextSource>,
        /// Nonempty ordered entry extractions applied to each item; the carrying `Vec1` type
        /// rejects an empty list at parse.
        #[cfg_attr(
            feature = "management-api-typescript-bindings",
            ts(as = "Vec<QuotaEntryExtraction>")
        )]
        entries: Vec1<QuotaEntryExtraction>,
    },
    /// An object whose members are the items, visited in lexicographic key order; each member
    /// name is the item's identity segment. The member key is available to the scope as
    /// `map_key`.
    Map {
        /// Stable source identity segment, unique among the descriptor's sources.
        key: String,
        /// Root-relative object-member path selecting the object.
        at: ResponsePath,
        /// Whether an absent source fails extraction; an absent optional source is omitted.
        is_required: bool,
        /// Optional named scope resolved per member, where `map_key` selects the member key;
        /// absent is the unknown scope.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "management-api-typescript-bindings", ts(optional))]
        scope: Option<QuotaTextSource>,
        /// Nonempty ordered entry extractions applied to each member value; the carrying
        /// `Vec1` type rejects an empty list at parse.
        #[cfg_attr(
            feature = "management-api-typescript-bindings",
            ts(as = "Vec<QuotaEntryExtraction>")
        )]
        entries: Vec1<QuotaEntryExtraction>,
    },
}

/// A nonempty text fact, identity segment, or scope name; the one text-source vocabulary the
/// array identity, the scope, and the routing selectors share.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaTextSource {
    /// Author-configured constant text.
    Literal {
        /// The exact text.
        value: String,
    },
    /// A text field read through its declared context and encoding; a present value of the
    /// wrong encoding fails the reading.
    Field {
        /// The scalar selection.
        field: QuotaField,
        /// Accepted text representation.
        encoding: QuotaTextEncoding,
    },
    /// The enclosing map source's member key; available only in a map context.
    MapKey,
}

/// The declared period; unknown is never inferred from a reset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaWindowSource {
    /// The period is unknown; a fixed-seconds, calendar-month, or duration-field mapping that
    /// never resolves becomes this unknown window.
    Unknown,
    /// A fixed positive whole-second allowance duration.
    Duration {
        /// The configured duration in seconds.
        seconds: SafeDuration,
    },
    /// A calendar period; a calendar month has no guessed alignment.
    Calendar {
        /// The calendar unit.
        unit: QuotaCalendarUnit,
    },
    /// A duration read from an upstream field through its declared numeric encoding and unit;
    /// the resolved magnitude must be a positive whole number of the declared unit.
    DurationField {
        /// The numeric field carrying the duration magnitude.
        value: QuotaNumberField,
        /// The unit the upstream magnitude is expressed in.
        unit: QuotaDurationUnit,
    },
}

/// The active quota measurement and its normalization direction. Used values may exceed limits;
/// remaining ratios must lie within their scale, and remaining counters cannot exceed their
/// positive limit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaMeasurementSource {
    /// The measurement is unknown; a mapping whose optional numeric fields are absent becomes
    /// this unknown measurement only after the present fields validate.
    Unknown,
    /// A fraction or percentage of an allowance, read in the declared direction.
    Ratio {
        /// The numeric field carrying the ratio magnitude.
        value: QuotaNumberField,
        /// The scale the magnitude is expressed on.
        scale: QuotaRatioScale,
        /// Whether the magnitude reports used or remaining allowance.
        direction: QuotaDirection,
    },
    /// A used or remaining amount against a positive limit, in the declared unit.
    Counter {
        /// The numeric field carrying the used or remaining amount.
        value: QuotaNumberField,
        /// The numeric field carrying the positive limit.
        limit: QuotaNumberField,
        /// Whether the amount reports used or remaining allowance.
        direction: QuotaDirection,
        /// The configured unit text of both amounts.
        unit: String,
    },
    /// A remaining-only amount without a declared limit, in the declared unit.
    Remaining {
        /// The numeric field carrying the remaining amount.
        value: QuotaNumberField,
        /// The configured unit text of the amount.
        unit: String,
    },
}

/// Reset metadata mapping without timestamp-format guessing: the encoding names the exact wire
/// representation, and a present value in any other representation fails the reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaResetSource {
    /// The reset instant is unknown.
    Unknown,
    /// The reset instant read from a field in one exact representation.
    Field {
        /// The scalar selection.
        field: QuotaField,
        /// Accepted reset representation.
        encoding: QuotaResetEncoding,
    },
}

/// The context object a [`QuotaField`] path resolves from.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaFieldBase {
    /// The complete decoded response root.
    Root,
    /// The item the enclosing source selected.
    Item,
    /// The object the enclosing entry's own `at` path addresses; unavailable while evaluating
    /// source scope or array identity.
    Entry,
}

/// Accepted text representation of a [`QuotaTextSource::Field`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaTextEncoding {
    /// A JSON string.
    String,
    /// A JSON integer, read by its exact spelling — never through floating-point conversion.
    Integer,
}

/// Accepted numeric wire representation of a [`QuotaNumberField`]: numeric strings use the JSON
/// number grammar without whitespace, signs other than a leading minus, or nonfinite spellings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaNumberEncoding {
    /// A JSON number only.
    Number,
    /// A strict JSON numeric string only.
    NumericString,
    /// Either a JSON number or a strict JSON numeric string.
    NumberOrNumericString,
}

/// Calendar allowance period.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaCalendarUnit {
    /// One calendar month, carrying no guessed alignment.
    Month,
}

/// Fixed duration unit of a [`QuotaWindowSource::DurationField`] magnitude.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaDurationUnit {
    /// Seconds.
    Seconds,
    /// Minutes.
    Minutes,
    /// Hours.
    Hours,
    /// Days.
    Days,
    /// Weeks.
    Weeks,
}

/// Ratio magnitude scale of a [`QuotaMeasurementSource::Ratio`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaRatioScale {
    /// A magnitude within `[0, 1]`.
    Fraction,
    /// A magnitude within `[0, 100]`.
    Percent,
}

/// Whether the upstream reports used or remaining allowance.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaDirection {
    /// The magnitude reports consumed allowance.
    Used,
    /// The magnitude reports remaining allowance.
    Remaining,
}

/// Exact reset representation of a [`QuotaResetSource::Field`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaResetEncoding {
    /// Whole Unix seconds; the instant keeps its exact value.
    UnixSeconds,
    /// Whole Unix milliseconds; the instant keeps its exact value.
    UnixMilliseconds,
    /// An RFC 3339 instant, rejecting any nonzero fractional digit beyond microseconds.
    Rfc3339,
    /// A calendar date without a time or timezone; never an absolute instant on its own.
    Date,
}
