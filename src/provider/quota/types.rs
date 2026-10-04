//! Validated numeric domains and concrete quota result facts.

use serde::{Deserialize, Serialize};

/// Largest integer exactly representable by the administration frontend.
pub(crate) const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// A finite, nonnegative informational amount, normalized to positive zero.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[cfg_attr(feature = "management-api-typescript-bindings", ts(type = "number"))]
pub struct Amount(f64);

impl TryFrom<f64> for Amount {
    type Error = &'static str;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if !value.is_finite() || value < 0.0 {
            return Err("amount must be finite and nonnegative");
        }
        Ok(Self(if value == 0.0 { 0.0 } else { value }))
    }
}

impl From<Amount> for f64 {
    fn from(value: Amount) -> Self {
        value.0
    }
}

/// A positive finite counter limit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[cfg_attr(feature = "management-api-typescript-bindings", ts(type = "number"))]
pub struct PositiveAmount(f64);

impl TryFrom<f64> for PositiveAmount {
    type Error = &'static str;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if !value.is_finite() || value <= 0.0 {
            return Err("limit must be finite and positive");
        }
        Ok(Self(value))
    }
}

impl From<PositiveAmount> for f64 {
    fn from(value: PositiveAmount) -> Self {
        value.0
    }
}

/// Positive whole seconds within the frontend's safe integer domain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[cfg_attr(feature = "management-api-typescript-bindings", ts(type = "number"))]
pub struct SafeDuration(u64);

impl TryFrom<u64> for SafeDuration {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 0 || value > MAX_SAFE_INTEGER {
            return Err("duration must be positive and safe");
        }
        Ok(Self(value))
    }
}

impl From<SafeDuration> for u64 {
    fn from(value: SafeDuration) -> Self {
        value.0
    }
}

/// UTC microseconds exactly representable by the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[cfg_attr(feature = "management-api-typescript-bindings", ts(type = "number"))]
pub struct SafeTimestamp(i64);

impl TryFrom<i64> for SafeTimestamp {
    type Error = &'static str;
    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value.unsigned_abs() > MAX_SAFE_INTEGER {
            return Err("timestamp must be safe");
        }
        Ok(Self(value))
    }
}

impl From<SafeTimestamp> for i64 {
    fn from(value: SafeTimestamp) -> Self {
        value.0
    }
}

/// One stable, independently observed upstream restriction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct QuotaEntry {
    /// Ordered identity tuple, never delimiter-joined.
    pub key: Vec<String>,
    /// Human-readable restriction label.
    pub label: String,
    /// Account, pool, or model scope.
    pub scope: QuotaScope,
    /// Resolved dispatch applicability, independent of presentation scope.
    pub routing: QuotaRouting,
    /// Observed allowance period.
    pub window: QuotaWindow,
    /// Observed normalized usage or remaining amount.
    pub measurement: QuotaMeasurement,
    /// Reset metadata without inferred alignment.
    pub reset: QuotaReset,
}

/// Concrete dispatch applicability of a normalized quota entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaRouting {
    /// No effect on account availability.
    MonitoringOnly,
    /// Restricts every exact model on this account.
    AllModels,
    /// Restricts only the resolved exact model IDs, sorted and deduplicated.
    Models {
        /// Nonempty canonical model set.
        upstream_ids: Vec<String>,
    },
}

/// Persistent applicability of an exhausted restriction; monitoring-only cannot inhabit it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaApplicability {
    /// Restricts every model on the account.
    AllModels,
    /// Restricts a canonical nonempty set of exact upstream IDs.
    Models {
        /// Exact model IDs in lexicographic order without duplicates.
        upstream_ids: Vec<String>,
    },
}

impl QuotaApplicability {
    /// Retains applicable observations and excludes monitoring-only measurements.
    pub(crate) fn from_routing(routing: &QuotaRouting) -> Option<Self> {
        match routing {
            QuotaRouting::MonitoringOnly => None,
            QuotaRouting::AllModels => Some(Self::AllModels),
            QuotaRouting::Models { upstream_ids } => Some(
                Self::Models {
                    upstream_ids: upstream_ids.clone(),
                }
                .canonicalized(),
            ),
        }
    }

    /// Canonicalizes an exact model set without changing the restriction's scope.
    pub(crate) fn canonicalized(mut self) -> Self {
        if let Self::Models { upstream_ids } = &mut self {
            upstream_ids.sort();
            upstream_ids.dedup();
        }
        self
    }

    /// Whether the restriction applies to the requested exact physical upstream model.
    pub(crate) fn is_applicable_to(&self, upstream_id: &str) -> bool {
        match self {
            Self::AllModels => true,
            Self::Models { upstream_ids } => upstream_ids.iter().any(|id| id == upstream_id),
        }
    }
}

/// Scope of the independently reported pool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaScope {
    /// No scope was observed.
    Unknown,
    /// An explicit upstream or configured scope.
    Named {
        /// Scope label, retained verbatim.
        name: String,
    },
}

/// Allowance period; an absolute reset never implies one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaWindow {
    /// The period is unknown.
    Unknown,
    /// A fixed positive duration.
    Duration {
        /// Whole seconds.
        seconds: SafeDuration,
    },
    /// A calendar period with unspecified alignment.
    Calendar {
        /// Calendar unit.
        unit: crate::provider::definition::quota::QuotaCalendarUnit,
    },
}

/// Normalized informational allowance measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaMeasurement {
    /// No measurement was observed.
    Unknown,
    /// A used fraction; over one remains visible.
    Ratio {
        /// Used allowance divided by its total.
        used_fraction: Amount,
    },
    /// An observed used amount and explicit positive limit.
    Counter {
        /// Used amount; may exceed the limit.
        used: Amount,
        /// Positive allowance limit.
        limit: PositiveAmount,
        /// Declared amount unit.
        unit: String,
    },
    /// Remaining amount with no inferred total.
    Remaining {
        /// Nonnegative remaining amount.
        remaining: Amount,
        /// Declared amount unit.
        unit: String,
    },
    /// Explicit upstream unlimited flag.
    Unlimited,
}

impl QuotaMeasurement {
    /// Positive availability evidence, exhaustion evidence, or unknown evidence.
    pub(crate) fn is_exhausted(&self) -> Option<bool> {
        match self {
            Self::Unknown => None,
            Self::Unlimited => Some(false),
            Self::Ratio { used_fraction } => Some(f64::from(*used_fraction) >= 1.0),
            Self::Counter { used, limit, .. } => Some(f64::from(*used) >= f64::from(*limit)),
            Self::Remaining { remaining, .. } => Some(f64::from(*remaining) == 0.0),
        }
    }
}

/// Reset information, independent of the allowance measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuotaReset {
    /// No reset was observed.
    Unknown,
    /// An exact instant, including a past one.
    At {
        /// UTC microseconds.
        at: SafeTimestamp,
    },
    /// A calendar date with unspecified timezone and time.
    Date {
        /// Validated date preserved as supplied.
        date: String,
    },
}
