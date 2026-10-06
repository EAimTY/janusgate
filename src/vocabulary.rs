//! The crate's closed cross-layer vocabularies: value domains every layer names, owned by no
//! single consumer.
//!
//! A vocabulary here depends on nothing but crate primitives, so the storage codec, the provider
//! definitions and their discovery, the format codecs, the management API, and the IR all import
//! it without crossing a layer boundary.

use serde::{Deserialize, Serialize};

/// Closed reasoning vocabulary shared by every format (README §6.4): one enum, forever.
///
/// Inbound surfaces parse their own parameter into this enum; unknown strings are a 400 at the
/// gateway. Upstream renderers write the enum verbatim into the target's own level parameter.
/// There is no cross-vocabulary mapping, and there never will be — a value the target does not
/// support is rejected by the upstream and mirrored to the client per the existing error policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ReasoningLevel {
    /// No reasoning; renders as the native off spelling per format (README §6.4).
    None,

    /// Minimal reasoning effort.
    Minimal,

    /// Low reasoning effort.
    Low,

    /// Medium reasoning effort.
    Medium,

    /// High reasoning effort.
    High,

    /// Extra-high reasoning effort.
    XHigh,

    /// Maximum reasoning effort.
    Max,
}

impl ReasoningLevel {
    /// The one (level, wire-spelling) table of the closed vocabulary (README §6.4): the
    /// [`Self::parse`] and [`Self::as_str`] matches and every format surface's conformance test
    /// pin derive from it, so a spelling change is a single edit here.
    #[cfg(test)]
    pub(crate) const VOCABULARY: [(ReasoningLevel, &'static str); 7] = [
        (Self::None, "none"),
        (Self::Minimal, "minimal"),
        (Self::Low, "low"),
        (Self::Medium, "medium"),
        (Self::High, "high"),
        (Self::XHigh, "xhigh"),
        (Self::Max, "max"),
    ];

    /// The vocabulary in canonical order — the storage and declaration order of a level set
    /// (README §3/§5.5): a stored or declared set is deduplicated into this order at write.
    pub(crate) const CANONICAL: [Self; 7] = [
        Self::None,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    /// Parses a vocabulary string domain-neutrally: [`Some`] for a member, [`None`] for a
    /// foreign string. The vocabulary is shared by the request boundary (which 400s on a
    /// foreign value through [`Self::parse`]), the discovery ingestion boundary (which skips a
    /// foreign level, README §5.5), and the management override boundary (which rejects one) —
    /// each naming the vocabulary in its own error domain.
    #[must_use]
    pub(crate) fn from_vocabulary_str(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::XHigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }

    /// Canonicalizes a set of levels: deduplicated, in [`Self::CANONICAL`] order. Both write
    /// paths that store a level set — discovery ingestion and the management override — pass
    /// through here, so a stored set is always the canonical form (README §3).
    #[must_use]
    pub(crate) fn canonicalize_levels(levels: impl IntoIterator<Item = Self>) -> Vec<Self> {
        let mut present = [false; 7];

        for level in levels {
            #[expect(
                clippy::indexing_slicing,
                reason = "the canonical index of a vocabulary member is within the table by \
                          construction"
            )]
            {
                present[Self::canonical_index(level)] = true;
            }
        }

        #[expect(
            clippy::indexing_slicing,
            reason = "every enumerated index of CANONICAL is within the table by construction"
        )]
        Self::CANONICAL
            .into_iter()
            .enumerate()
            .filter(|(index, _)| present[*index])
            .map(|(_, level)| level)
            .collect()
    }

    /// The level's index in [`Self::CANONICAL`].
    const fn canonical_index(level: Self) -> usize {
        match level {
            Self::None => 0,
            Self::Minimal => 1,
            Self::Low => 2,
            Self::Medium => 3,
            Self::High => 4,
            Self::XHigh => 5,
            Self::Max => 6,
        }
    }

    /// Renders the canonical vocabulary string, written verbatim into the target's own level
    /// parameter.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ReasoningLevel;

    /// Pins the [`ReasoningLevel::as_str`] match to [`ReasoningLevel::VOCABULARY`]: every
    /// entry's wire spelling is exactly what `as_str` renders, and the spelling parses back
    /// domain-neutrally to the same level. The request-boundary `parse` wrapper is pinned
    /// against the same table beside its rejection type in [`crate::ir`].
    ///
    /// # Panics
    ///
    /// Panics if a table entry diverges from either match.
    #[test]
    fn vocabulary_round_trips_through_as_str_and_from_vocabulary_str() {
        for (level, spelling) in ReasoningLevel::VOCABULARY {
            assert_eq!(level.as_str(), spelling);

            assert_eq!(
                ReasoningLevel::from_vocabulary_str(spelling),
                Some(level),
                "the vocabulary spelling must parse domain-neutrally",
            );
        }
    }
}
