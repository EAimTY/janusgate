//! The closed vocabulary of remote-compaction mechanisms and their support
//! states.
//!
//! A *mechanism* is one client-visible compaction operation family. A format's
//! ability to encode a mechanism is not evidence that a particular upstream
//! supports it: support is resolved per selected model from an administrator
//! override, then a discovery-owned model fact, then the provider definition's
//! own declaration; otherwise the mechanism is [`MechanismSupport::Unknown`],
//! which selects gateway summarization without a speculative native call.

use serde::{Deserialize, Serialize};

/// One client-visible remote-compaction operation family.
///
/// The derived order is the declaration order — the canonical order of
/// [`Self::ALL`], which the support-facts map iterates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mechanism {
    /// The Responses standalone compact endpoint (`POST /responses/compact`).
    ResponsesStandaloneCompact,

    /// An explicit `compaction_trigger` control item in a Responses request.
    ResponsesExplicitTrigger,

    /// Responses automatic threshold compaction through `context_management`.
    ResponsesAutomaticCompaction,

    /// Messages on-demand compaction (`compaction: {"type": "summarize"}`).
    MessagesOnDemandCompaction,

    /// Messages threshold compaction through `context_management.edits`.
    MessagesThresholdCompaction,
}

impl Mechanism {
    /// Every mechanism in declaration order.
    pub(crate) const ALL: [Self; 5] = [
        Self::ResponsesStandaloneCompact,
        Self::ResponsesExplicitTrigger,
        Self::ResponsesAutomaticCompaction,
        Self::MessagesOnDemandCompaction,
        Self::MessagesThresholdCompaction,
    ];

    /// The mechanism's canonical storage and wire spelling.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ResponsesStandaloneCompact => "responses_standalone_compact",
            Self::ResponsesExplicitTrigger => "responses_explicit_trigger",
            Self::ResponsesAutomaticCompaction => "responses_automatic_compaction",
            Self::MessagesOnDemandCompaction => "messages_on_demand_compaction",
            Self::MessagesThresholdCompaction => "messages_threshold_compaction",
        }
    }

    /// Parses the canonical spelling.
    #[must_use]
    pub(crate) fn from_str(spelling: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mechanism| mechanism.as_str() == spelling)
    }
}

/// A mechanism's support state at one resolution layer.
///
/// Unknown is a positive state meaning "no fact": it never selects a native
/// call, and discovery/preset edits must preserve it distinctly from
/// unsupported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum MechanismSupport {
    /// The upstream is known to support the mechanism.
    Supported,

    /// The upstream is known not to support the mechanism.
    Unsupported,

    /// Nothing is known about the mechanism's support.
    #[default]
    Unknown,
}

impl MechanismSupport {
    /// Parses the canonical spelling.
    #[must_use]
    pub(crate) fn from_str(spelling: &str) -> Option<Self> {
        match spelling {
            "supported" => Some(Self::Supported),
            "unsupported" => Some(Self::Unsupported),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::missing_panics_doc,
        reason = "a test fails by panicking on its own assertions; a # Panics section on every \
                  test would document nothing its name does not"
    )]

    use super::{Mechanism, MechanismSupport};

    /// Every mechanism's canonical spelling parses back to itself, the declared order is the
    /// closed `ALL` order, and an unknown spelling parses to nothing.
    #[test]
    fn mechanism_spellings_round_trip_in_declaration_order() {
        let canonical = [
            "responses_standalone_compact",
            "responses_explicit_trigger",
            "responses_automatic_compaction",
            "messages_on_demand_compaction",
            "messages_threshold_compaction",
        ];

        for (mechanism, spelling) in Mechanism::ALL.into_iter().zip(canonical) {
            assert_eq!(mechanism.as_str(), spelling);

            assert_eq!(Mechanism::from_str(spelling), Some(mechanism));
        }

        assert_eq!(Mechanism::from_str("responses_compact"), None);

        assert_eq!(Mechanism::from_str(""), None);
    }

    /// The support vocabulary parses exactly its three canonical spellings, and the absent-fact
    /// default is `unknown` — the positive no-fact state that must stay distinct from
    /// `unsupported`.
    #[test]
    fn support_vocabulary_parses_and_defaults_to_unknown() {
        assert_eq!(
            MechanismSupport::from_str("supported"),
            Some(MechanismSupport::Supported)
        );

        assert_eq!(
            MechanismSupport::from_str("unsupported"),
            Some(MechanismSupport::Unsupported)
        );

        assert_eq!(
            MechanismSupport::from_str("unknown"),
            Some(MechanismSupport::Unknown)
        );

        assert_eq!(MechanismSupport::from_str("Supported"), None);

        assert_eq!(MechanismSupport::from_str(""), None);

        assert_eq!(MechanismSupport::default(), MechanismSupport::Unknown);
    }
}
