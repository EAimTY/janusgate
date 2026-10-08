//! Public model targets and ordered account membership, independent of physical inventory.

use serde::{Deserialize, Serialize};

/// An explicitly published model route's target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(crate) enum ModelRouteTarget {
    /// One concrete physical model; bypasses pooled selection and affinity.
    Direct {
        /// Physical model row identifier.
        model_id: i64,
    },
    /// Interchangeable accounts serving one exact upstream model identifier.
    Pool {
        /// Account pool row identifier.
        pool_id: i64,
        /// Exact upstream identifier, never an alias or a synthesized inventory row.
        upstream_id: String,
    },
}

/// One account's position in a pool's authored total order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(crate) struct ProviderPoolMember {
    /// Credential-bearing account row identifier.
    pub(crate) provider_id: i64,
    /// Nonnegative order key; smaller values are considered first.
    pub(crate) priority: u32,
}
