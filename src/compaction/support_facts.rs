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

#[cfg(test)]
mod tests {
    use super::{CompactionSupportFacts, Mechanism, MechanismSupport, resolve_mechanism};
    use MechanismSupport::{Supported, Unknown, Unsupported};

    /// Builds one two-member fact map in canonical order.
    ///
    /// # Panics
    ///
    /// Panics if the fixture pairs are empty, which every caller prevents.
    #[expect(
        clippy::expect_used,
        reason = "the fixture pairs are constructed non-empty"
    )]
    fn facts(pairs: &[(Mechanism, MechanismSupport)]) -> CompactionSupportFacts {
        CompactionSupportFacts::from_pairs(pairs.iter().copied())
            .expect("the fixture pairs are non-empty")
    }

    /// Pins the JSON wire shape: the map renders as one object keyed by the mechanism
    /// spellings in canonical order, the support values as their spellings, and parsing admits
    /// the explicit `unknown` value the override layer stores.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn renders_and_parses_the_object_shape() {
        let facts = facts(&[
            (Mechanism::ResponsesStandaloneCompact, Supported),
            (Mechanism::MessagesOnDemandCompaction, Unsupported),
            (Mechanism::MessagesThresholdCompaction, Unknown),
        ]);

        #[expect(clippy::expect_used, reason = "the facts form always serializes")]
        let rendered = serde_json::to_value(&facts).expect("the facts serialize");

        assert_eq!(
            rendered,
            serde_json::json!({
                "responses_standalone_compact": "supported",
                "messages_on_demand_compaction": "unsupported",
                "messages_threshold_compaction": "unknown",
            })
        );

        #[expect(
            clippy::expect_used,
            reason = "the rendered form is constructed to parse"
        )]
        let parsed: CompactionSupportFacts =
            serde_json::from_value(rendered).expect("the rendered facts parse");

        assert_eq!(parsed, facts);

        // A foreign mechanism spelling and a foreign support value are both parse rejections.
        assert!(
            serde_json::from_value::<CompactionSupportFacts>(serde_json::json!({
                "bogus_mechanism": "supported",
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<CompactionSupportFacts>(serde_json::json!({
                "messages_on_demand_compaction": "partially",
            }))
            .is_err()
        );
    }

    /// The empty map is the nothing-known state: [`CompactionSupportFacts::from_pairs`]
    /// normalizes it to `None`, which the storage writers persist as `NULL`.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn normalizes_the_empty_map_to_none() {
        assert_eq!(CompactionSupportFacts::from_pairs([]), None);
        assert_eq!(
            CompactionSupportFacts::from_pairs([(Mechanism::MessagesOnDemandCompaction, Unknown)]),
            Some(facts(&[(Mechanism::MessagesOnDemandCompaction, Unknown)])),
        );

        let single = facts(&[(Mechanism::MessagesOnDemandCompaction, Supported)]);

        assert!(single.get(Mechanism::MessagesThresholdCompaction).is_none(),);
        assert_eq!(
            single.get(Mechanism::MessagesOnDemandCompaction),
            Some(Supported),
        );
        assert!(!single.carries_unknown());
        assert!(facts(&[(Mechanism::MessagesThresholdCompaction, Unknown)]).carries_unknown(),);
    }

    /// Pins the per-member resolution order: the override, then the base, then the definition
    /// default — an absent member skipping its layer, and an explicit `unknown` override
    /// suppressing both lower layers.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn resolves_per_member_in_layer_order() {
        let base = facts(&[
            (Mechanism::ResponsesExplicitTrigger, Unsupported),
            (Mechanism::MessagesOnDemandCompaction, Supported),
        ]);
        let override_ = facts(&[
            (Mechanism::MessagesOnDemandCompaction, Unknown),
            (Mechanism::MessagesThresholdCompaction, Supported),
        ]);

        // The explicit unknown override suppresses the base's supported fact into nothing known.
        assert_eq!(
            resolve_mechanism(
                Supported,
                Some(&base),
                Some(&override_),
                Mechanism::MessagesOnDemandCompaction,
            ),
            Unknown,
        );

        // A fact the override does not mention falls through to the base.
        assert_eq!(
            resolve_mechanism(
                Supported,
                Some(&base),
                Some(&override_),
                Mechanism::ResponsesExplicitTrigger,
            ),
            Unsupported,
        );

        // A fact only the override carries wins over the definition default.
        assert_eq!(
            resolve_mechanism(
                Unsupported,
                None,
                Some(&override_),
                Mechanism::MessagesThresholdCompaction,
            ),
            Supported,
        );

        // A mechanism neither model layer carries falls to the definition default.
        assert_eq!(
            resolve_mechanism(
                Unsupported,
                Some(&base),
                Some(&override_),
                Mechanism::ResponsesStandaloneCompact,
            ),
            Unsupported,
        );
        assert_eq!(
            resolve_mechanism(
                Unknown,
                Some(&base),
                Some(&override_),
                Mechanism::ResponsesStandaloneCompact,
            ),
            Unknown,
        );
    }
}
