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

#[cfg(test)]
mod tests {
    //! Tests for the public-route target wire contract: the typed target tags and members the
    //! management surface carries, and the nonnegative membership priority domain
    //! (`docs/design/routing.md` *Names and ownership*, *Ordered pool selection*).

    #![expect(
        clippy::missing_panics_doc,
        reason = "a test fails by panicking; a # Panics section on every test would restate that"
    )]

    use super::{ModelRouteTarget, ProviderPoolMember};

    /// A target's wire tag names its kind and carries exactly its own members: direct targets
    /// name `model_id`, pooled targets name `pool_id` plus the exact `upstream_id`, and unknown
    /// members are rejected.
    #[test]
    fn target_wire_tags_carry_their_own_members() -> Result<(), Box<dyn std::error::Error>> {
        let direct: ModelRouteTarget = serde_json::from_value(serde_json::json!({
            "type": "direct",
            "model_id": 17,
        }))?;

        assert_eq!(direct, ModelRouteTarget::Direct { model_id: 17 });

        let pooled: ModelRouteTarget = serde_json::from_value(serde_json::json!({
            "type": "pool",
            "pool_id": 3,
            "upstream_id": "gemini-2.5-flash",
        }))?;

        assert_eq!(
            pooled,
            ModelRouteTarget::Pool {
                pool_id: 3,
                upstream_id: "gemini-2.5-flash".to_string(),
            }
        );

        assert!(
            serde_json::from_value::<ModelRouteTarget>(serde_json::json!({
                "type": "direct",
                "model_id": 17,
                "upstream_id": "leaked",
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ModelRouteTarget>(serde_json::json!({
                "type": "pooled",
                "pool_id": 3,
                "upstream_id": "x",
            }))
            .is_err()
        );

        Ok(())
    }

    /// A membership position carries the account and its nonnegative priority; a negative
    /// priority is outside the domain and rejected on the wire.
    #[test]
    fn membership_priorities_are_nonnegative() -> Result<(), Box<dyn std::error::Error>> {
        let member: ProviderPoolMember = serde_json::from_value(serde_json::json!({
            "provider_id": 5,
            "priority": 0,
        }))?;

        assert_eq!(member.provider_id, 5);
        assert_eq!(member.priority, 0);

        assert!(
            serde_json::from_value::<ProviderPoolMember>(serde_json::json!({
                "provider_id": 5,
                "priority": -1,
            }))
            .is_err()
        );

        Ok(())
    }
}
