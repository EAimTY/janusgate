//! Sparse API key permissions over stable public-route and typed owner identities.

use serde::{Deserialize, Serialize};

/// An authored permission decision; inheritance is absence, never a third decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(crate) enum AccessDecision {
    /// Authorize this public entry subject to global availability.
    Allow,
    /// Hide this public entry from discovery and invocation.
    Deny,
}

impl AccessDecision {
    /// Canonical database spelling; storage owns the decode side of this vocabulary.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

/// A stable public route's owner; equal numerical IDs of different kinds are unrelated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RouteOwner {
    /// Physical provider of a Direct target.
    Provider(i64),
    /// Pool of a Pool target, independent of its members.
    Pool(i64),
}

#[cfg(test)]
impl RouteOwner {
    /// Creates an explicit owner default with its typed identity.
    pub(crate) fn rule(self, access: AccessDecision) -> ApiKeyOwnerDefault {
        match self {
            Self::Provider(provider_id) => ApiKeyOwnerDefault::Provider {
                provider_id,
                access,
            },
            Self::Pool(pool_id) => ApiKeyOwnerDefault::Pool { pool_id, access },
        }
    }
}

/// One explicit default for a provider or pool, including owners with no public routes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(crate) enum ApiKeyOwnerDefault {
    /// Default inherited by Direct entries targeting this provider's models.
    Provider {
        /// Stable provider identity.
        provider_id: i64,
        /// Explicit decision, preserved even when equal to the key default.
        access: AccessDecision,
    },
    /// Default inherited by entries targeting this pool.
    Pool {
        /// Stable pool identity.
        pool_id: i64,
        /// Explicit decision for the whole pool entry.
        access: AccessDecision,
    },
}

impl ApiKeyOwnerDefault {
    /// Typed identity and authored decision.
    pub(crate) fn parts(&self) -> (RouteOwner, AccessDecision) {
        match *self {
            Self::Provider {
                provider_id,
                access,
            } => (RouteOwner::Provider(provider_id), access),
            Self::Pool { pool_id, access } => (RouteOwner::Pool(pool_id), access),
        }
    }
}

/// An explicit decision attached to a surviving public-route identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(crate) struct ApiKeyRouteOverride {
    /// Never-recycled public-route identity.
    pub model_route_id: i64,
    /// Explicit decision, independent of the route's current owner.
    pub access: AccessDecision,
}

/// The complete authored key policy; sparse collections are unordered on writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(crate) struct ApiKeyAccessControl {
    /// Decision for entries inheriting from the key.
    pub default_access: AccessDecision,
    /// Explicit typed owner defaults.
    pub owner_defaults: Vec<ApiKeyOwnerDefault>,
    /// Explicit public-route decisions.
    pub route_overrides: Vec<ApiKeyRouteOverride>,
}

impl ApiKeyAccessControl {
    /// Resolves route → owner → key without inspecting global gates or pool members.
    pub(crate) fn resolve(&self, model_route_id: i64, owner: RouteOwner) -> AccessDecision {
        self.route_overrides
            .iter()
            .find(|rule| rule.model_route_id == model_route_id)
            .map(|rule| rule.access)
            .or_else(|| {
                self.owner_defaults.iter().find_map(|rule| {
                    let (identity, access) = rule.parts();
                    (identity == owner).then_some(access)
                })
            })
            .unwrap_or(self.default_access)
    }

    /// Canonical readable ordering; explicit equal-default entries are retained.
    pub(crate) fn canonicalize(&mut self) {
        self.owner_defaults
            .sort_by_key(|rule| match rule.parts().0 {
                RouteOwner::Provider(id) => (0, id),
                RouteOwner::Pool(id) => (1, id),
            });
        self.route_overrides.sort_by_key(|rule| rule.model_route_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every level wins over lower levels, with distinct provider and pool identities.
    ///
    /// # Panics
    /// Panics if precedence, explicit pins, or owner-kind isolation changes.
    #[test]
    fn precedence_and_explicit_intent() {
        for default_access in [AccessDecision::Allow, AccessDecision::Deny] {
            for owner_access in [
                None,
                Some(AccessDecision::Allow),
                Some(AccessDecision::Deny),
            ] {
                for route_access in [
                    None,
                    Some(AccessDecision::Allow),
                    Some(AccessDecision::Deny),
                ] {
                    for owner in [RouteOwner::Provider(7), RouteOwner::Pool(7)] {
                        let mut policy = ApiKeyAccessControl {
                            default_access,
                            owner_defaults: owner_access
                                .map(|access| owner.rule(access))
                                .into_iter()
                                .collect(),
                            route_overrides: route_access
                                .map(|access| ApiKeyRouteOverride {
                                    model_route_id: 41,
                                    access,
                                })
                                .into_iter()
                                .collect(),
                        };
                        assert_eq!(
                            policy.resolve(41, owner),
                            route_access.or(owner_access).unwrap_or(default_access)
                        );
                        assert_eq!(
                            policy.resolve(42, owner),
                            owner_access.unwrap_or(default_access)
                        );
                        let other = match owner {
                            RouteOwner::Provider(id) => RouteOwner::Pool(id),
                            RouteOwner::Pool(id) => RouteOwner::Provider(id),
                        };
                        assert_eq!(policy.resolve(42, other), default_access);
                        policy.default_access = if default_access == AccessDecision::Allow {
                            AccessDecision::Deny
                        } else {
                            AccessDecision::Allow
                        };
                        assert_eq!(
                            policy.resolve(41, owner),
                            route_access
                                .or(owner_access)
                                .unwrap_or(policy.default_access)
                        );
                    }
                }
            }
        }
    }
}
