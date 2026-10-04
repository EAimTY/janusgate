//! Product-independent quota descriptor vocabulary (README §5.10).

use super::template::{ControlRequest, ResponsePath};
use crate::provider::quota::SafeDuration;
use serde::{Deserialize, Serialize};

/// One read-only quota query and its ordered extraction sources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct QuotaDescriptor {
    /// The authenticated control request.
    pub request: ControlRequest,
    /// Nonempty ordered sources, validated at the management write boundary.
    pub sources: Vec<QuotaSource>,
}

/// One independently identified allowance within a source item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct QuotaEntryExtraction {
    /// Stable entry identity segment.
    pub key: String,
    /// Human-readable restriction name.
    pub label: String,
    /// Object path relative to the selected item.
    pub at: ResponsePath,
    /// Whether an absent entry fails extraction.
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
    /// Optional explicit boolean that bypasses numeric measurement extraction.
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
    /// Applies only to the nonempty set of exact IDs resolved in this entry's contexts.
    Models {
        /// Required text sources; unresolved values invalidate the complete observation.
        upstream_ids: Vec<QuotaTextSource>,
    },
}

/// A scalar selection with an explicit response context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct QuotaField {
    /// The context from which the path resolves.
    pub base: QuotaFieldBase,
    /// Object-member chain; empty selects the context itself.
    pub path: ResponsePath,
    /// Whether absence fails extraction.
    pub is_required: bool,
}

/// A numeric field with an explicit wire encoding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct QuotaNumberField {
    /// The scalar selection.
    pub field: QuotaField,
    /// Accepted numeric representation.
    pub encoding: QuotaNumberEncoding,
}

/// One source collection; its order and identity determine result order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaSource {
    /// The Object mapping.
    Object {
        /// The configured key.
        key: String,
        /// The configured at.
        at: ResponsePath,
        /// The configured is required.
        is_required: bool,
        /// The configured scope.
        scope: QuotaScopeSource,
        /// The configured entries.
        entries: Vec<QuotaEntryExtraction>,
    },
    /// The Array mapping.
    Array {
        /// The configured key.
        key: String,
        /// The configured at.
        at: ResponsePath,
        /// The configured is required.
        is_required: bool,
        /// The configured identity.
        identity: Vec<QuotaTextSource>,
        /// The configured scope.
        scope: QuotaScopeSource,
        /// The configured entries.
        entries: Vec<QuotaEntryExtraction>,
    },
    /// The Map mapping.
    Map {
        /// The configured key.
        key: String,
        /// The configured at.
        at: ResponsePath,
        /// The configured is required.
        is_required: bool,
        /// The configured scope.
        scope: QuotaScopeSource,
        /// The configured entries.
        entries: Vec<QuotaEntryExtraction>,
    },
}

/// A nonempty text fact or identity segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaTextSource {
    /// The Literal mapping.
    Literal {
        /// The configured value.
        value: String,
    },
    /// The Field mapping.
    Field {
        /// The configured field.
        field: QuotaField,
        /// The configured encoding.
        encoding: QuotaTextEncoding,
    },
    /// The MapKey mapping.
    MapKey,
}

/// An optional named scope, independent of the restriction label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaScopeSource {
    /// The Unknown mapping.
    Unknown,
    /// The Literal mapping.
    Literal {
        /// The configured value.
        value: String,
    },
    /// The Field mapping.
    Field {
        /// The configured field.
        field: QuotaField,
        /// The configured encoding.
        encoding: QuotaTextEncoding,
    },
    /// The MapKey mapping.
    MapKey,
}

/// The declared period; unknown is never inferred from a reset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaWindowSource {
    /// The Unknown mapping.
    Unknown,
    /// The Duration mapping.
    Duration {
        /// The configured seconds.
        seconds: SafeDuration,
    },
    /// The Calendar mapping.
    Calendar {
        /// The configured unit.
        unit: QuotaCalendarUnit,
    },
    /// The DurationField mapping.
    DurationField {
        /// The configured value.
        value: QuotaNumberField,
        /// The configured unit.
        unit: QuotaDurationUnit,
    },
}

/// The active quota measurement and its normalization direction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaMeasurementSource {
    /// The Unknown mapping.
    Unknown,
    /// The Ratio mapping.
    Ratio {
        /// The configured value.
        value: QuotaNumberField,
        /// The configured scale.
        scale: QuotaRatioScale,
        /// The configured direction.
        direction: QuotaDirection,
    },
    /// The Counter mapping.
    Counter {
        /// The configured value.
        value: QuotaNumberField,
        /// The configured limit.
        limit: QuotaNumberField,
        /// The configured direction.
        direction: QuotaDirection,
        /// The configured unit.
        unit: String,
    },
    /// The Remaining mapping.
    Remaining {
        /// The configured value.
        value: QuotaNumberField,
        /// The configured unit.
        unit: String,
    },
}

/// Reset metadata mapping without timestamp-format guessing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaResetSource {
    /// The Unknown mapping.
    Unknown,
    /// The Field mapping.
    Field {
        /// The configured field.
        field: QuotaField,
        /// The configured encoding.
        encoding: QuotaResetEncoding,
    },
}

/// Scalar selection context.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaFieldBase {
    /// The Root representation.
    Root,
    /// The Item representation.
    Item,
    /// The Entry representation.
    Entry,
}

/// Identity text representation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaTextEncoding {
    /// The String representation.
    String,
    /// The Integer representation.
    Integer,
}

/// Accepted numeric wire representation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaNumberEncoding {
    /// The Number representation.
    Number,
    /// The NumericString representation.
    NumericString,
    /// The NumberOrNumericString representation.
    NumberOrNumericString,
}

/// Calendar allowance period.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaCalendarUnit {
    /// The Month representation.
    Month,
}

/// Fixed duration unit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaDurationUnit {
    /// The Seconds representation.
    Seconds,
    /// The Minutes representation.
    Minutes,
    /// The Hours representation.
    Hours,
    /// The Days representation.
    Days,
    /// The Weeks representation.
    Weeks,
}

/// Ratio magnitude scale.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaRatioScale {
    /// The Fraction representation.
    Fraction,
    /// The Percent representation.
    Percent,
}

/// Whether the upstream reports used or remaining allowance.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaDirection {
    /// The Used representation.
    Used,
    /// The Remaining representation.
    Remaining,
}

/// Explicit reset representation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaResetEncoding {
    /// The UnixSeconds representation.
    UnixSeconds,
    /// The UnixMilliseconds representation.
    UnixMilliseconds,
    /// The Rfc3339 representation.
    Rfc3339,
    /// The Date representation.
    Date,
}
