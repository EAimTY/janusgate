//! The per-model compaction support facts: an ordered mechanism-to-support map stored as one
//! JSON object on the model rows.
//!
//! The map is the storage and wire form of two model-row columns (`docs/design/storage.md`'s
//! capability-column ownership classes): the discovery-owned base and the administrator-owned
//! override. The base column admits only known facts (`supported`/`unsupported`) — discovery
//! extracts booleans, so it never produces `unknown` — while the override additionally admits an
//! explicit `unknown` that suppresses every lower resolution layer for that mechanism. An absent
//! member and a `NULL` column both mean *nothing known at that layer*; the domains themselves are
//! enforced at the write boundaries, never in SQL.

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

#[cfg(test)]
mod tests {
    #![expect(
        clippy::missing_panics_doc,
        reason = "the tests' assertions panic; a # Panics section on every test would \
                  restate that"
    )]

    use super::{CompactionSupportFacts, resolve_mechanism};
    use crate::compaction_model::mechanism::{Mechanism, MechanismSupport};
    use std::error::Error as StdError;

    /// An empty pair set is the nothing-known state — `None`, the form every producer stores as
    /// `NULL` — while any fact yields a map that answers its members and no others.
    #[test]
    fn empty_pairs_are_the_nothing_known_state() -> Result<(), Box<dyn StdError>> {
        assert_eq!(CompactionSupportFacts::from_pairs(Vec::new()), None);

        let facts = CompactionSupportFacts::from_pairs([(
            Mechanism::MessagesOnDemandCompaction,
            MechanismSupport::Supported,
        )])
        .ok_or("a non-empty pair set yields facts")?;

        assert_eq!(
            facts.get(Mechanism::MessagesOnDemandCompaction),
            Some(MechanismSupport::Supported)
        );

        assert_eq!(facts.get(Mechanism::ResponsesStandaloneCompact), None);

        Ok(())
    }

    /// `carries_unknown` distinguishes the explicit suppression fact from known states, and the
    /// canonical iteration order is the declaration order of `Mechanism::ALL` regardless of
    /// construction order.
    #[test]
    fn unknown_detection_and_canonical_order() -> Result<(), Box<dyn StdError>> {
        let facts = CompactionSupportFacts::from_pairs([
            (
                Mechanism::MessagesThresholdCompaction,
                MechanismSupport::Unsupported,
            ),
            (
                Mechanism::ResponsesStandaloneCompact,
                MechanismSupport::Supported,
            ),
        ])
        .ok_or("a non-empty pair set yields facts")?;

        assert!(!facts.carries_unknown());

        let ordered: Vec<_> = facts.iter().map(|(mechanism, _)| mechanism).collect();

        assert_eq!(
            ordered,
            [
                Mechanism::ResponsesStandaloneCompact,
                Mechanism::MessagesThresholdCompaction
            ],
            "the map iterates in canonical mechanism order"
        );

        let suppressed = CompactionSupportFacts::from_pairs([(
            Mechanism::MessagesThresholdCompaction,
            MechanismSupport::Unknown,
        )])
        .ok_or("a non-empty pair set yields facts")?;

        assert!(suppressed.carries_unknown());

        Ok(())
    }

    /// The storage form is the bare JSON object keyed by the canonical mechanism spellings; it
    /// round-trips transparently in both serde directions.
    #[test]
    fn storage_form_is_the_bare_canonical_object() -> Result<(), Box<dyn StdError>> {
        let facts = CompactionSupportFacts::from_pairs([
            (
                Mechanism::ResponsesStandaloneCompact,
                MechanismSupport::Supported,
            ),
            (
                Mechanism::MessagesOnDemandCompaction,
                MechanismSupport::Unsupported,
            ),
        ])
        .ok_or("a non-empty pair set yields facts")?;

        let body = serde_json::to_value(&facts)?;

        assert_eq!(
            body,
            serde_json::json!({
                "responses_standalone_compact": "supported",
                "messages_on_demand_compaction": "unsupported"
            })
        );

        assert_eq!(
            serde_json::from_value::<CompactionSupportFacts>(body)?,
            facts
        );

        Ok(())
    }

    /// The three resolution layers apply in order — override, then discovery-owned base, then the
    /// definition default — and an explicit `unknown` override suppresses both lower layers for
    /// that mechanism alone.
    #[test]
    fn resolution_layers_apply_in_order_with_unknown_suppression() -> Result<(), Box<dyn StdError>>
    {
        let mechanism = Mechanism::ResponsesStandaloneCompact;

        let base = CompactionSupportFacts::from_pairs([
            (
                Mechanism::ResponsesStandaloneCompact,
                MechanismSupport::Supported,
            ),
            (
                Mechanism::MessagesOnDemandCompaction,
                MechanismSupport::Supported,
            ),
        ])
        .ok_or("a non-empty pair set yields facts")?;

        let override_ = CompactionSupportFacts::from_pairs([(
            Mechanism::ResponsesStandaloneCompact,
            MechanismSupport::Unknown,
        )])
        .ok_or("a non-empty pair set yields facts")?;

        assert_eq!(
            resolve_mechanism(MechanismSupport::Unsupported, None, None, mechanism),
            MechanismSupport::Unsupported,
            "the definition default answers when no stored layer knows the mechanism"
        );

        assert_eq!(
            resolve_mechanism(MechanismSupport::Unsupported, Some(&base), None, mechanism),
            MechanismSupport::Supported,
            "a discovered base fact beats the definition default"
        );

        assert_eq!(
            resolve_mechanism(
                MechanismSupport::Unsupported,
                Some(&base),
                Some(&override_),
                mechanism
            ),
            MechanismSupport::Unknown,
            "an explicit unknown override suppresses both lower layers"
        );

        assert_eq!(
            resolve_mechanism(
                MechanismSupport::Unsupported,
                Some(&base),
                Some(&override_),
                Mechanism::MessagesOnDemandCompaction
            ),
            MechanismSupport::Supported,
            "the suppression covers only the overridden mechanism"
        );

        Ok(())
    }
}
