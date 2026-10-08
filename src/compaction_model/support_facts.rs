//! The per-model compaction support facts: an ordered mechanism-to-support map stored as one
//! JSON object on the model rows.
//!
//! The map is the storage and wire form of two model-row columns (README §3's capability-column
//! ownership classes): the discovery-owned base and the administrator-owned override. The base
//! column admits only known facts (`supported`/`unsupported`) — discovery extracts booleans, so
//! it never produces `unknown` — while the override additionally admits an explicit `unknown`
//! that suppresses every lower resolution layer for that mechanism. An absent member and a
//! `NULL` column both mean *nothing known at that layer*; the domains themselves are enforced at
//! the write boundaries, never in SQL.

use super::mechanism::{Mechanism, MechanismSupport};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One model row's compaction support facts: an ordered map from mechanism to support state,
/// serialized as the JSON object `{"<mechanism>": "<support>", ...}`. The newtype is
/// transparent in both serde directions, so the wire and storage forms are the bare object.
///
/// The iteration order is the canonical [`Mechanism::ALL`] order (the derived enum order), so a
/// stored or rendered object is always canonically spelled. The map never carries an unknown
/// mechanism; whether its values may include [`MechanismSupport::Unknown`] is the owning
/// column's contract, not this type's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CompactionSupportFacts(BTreeMap<Mechanism, MechanismSupport>);

impl CompactionSupportFacts {
    /// Builds the facts from the given pairs, answering `None` when no fact remains: an empty
    /// map is the nothing-known state, which every producer stores as `NULL`.
    #[must_use]
    pub(crate) fn from_pairs(
        pairs: impl IntoIterator<Item = (Mechanism, MechanismSupport)>,
    ) -> Option<Self> {
        let map: BTreeMap<_, _> = pairs.into_iter().collect();

        (!map.is_empty()).then_some(Self(map))
    }

    /// The support state one mechanism carries, or `None` when the map carries no fact for it.
    #[must_use]
    pub(crate) fn get(&self, mechanism: Mechanism) -> Option<MechanismSupport> {
        self.0.get(&mechanism).copied()
    }

    /// Whether any member carries [`MechanismSupport::Unknown`] — the value the base column's
    /// parse rejects as drift and the override column's parse admits as an explicit suppression.
    #[must_use]
    pub(crate) fn carries_unknown(&self) -> bool {
        self.0
            .values()
            .any(|support| *support == MechanismSupport::Unknown)
    }

    /// The facts in canonical mechanism order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (Mechanism, MechanismSupport)> {
        self.0
            .iter()
            .map(|(mechanism, support)| (*mechanism, *support))
    }
}

/// Resolves one mechanism's effective support across the three layers: the model's
/// administrator override, then the discovery-owned model fact, then the definition's declared
/// default. Each layer's absent fact is `None`; a present layer wins outright, so an explicit
/// `unknown` override suppresses both lower layers into the unknown state.
#[must_use]
pub(crate) fn resolve_mechanism(
    definition_default: MechanismSupport,
    base: Option<&CompactionSupportFacts>,
    override_: Option<&CompactionSupportFacts>,
    mechanism: Mechanism,
) -> MechanismSupport {
    override_
        .and_then(|facts| facts.get(mechanism))
        .or_else(|| base.and_then(|facts| facts.get(mechanism)))
        .unwrap_or(definition_default)
}
