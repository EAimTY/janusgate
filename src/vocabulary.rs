//! The crate's closed cross-layer vocabularies: value domains every layer names, owned by no
//! single consumer.
//!
//! A vocabulary here depends on nothing but crate primitives, so the storage codec, the provider
//! definitions and their discovery, the format codecs, the management API, and the IR all import
//! it without crossing a layer boundary.

use serde::{Deserialize, Serialize};

/// Closed reasoning vocabulary shared by every format (`docs/reference/inference.md`): one enum,
/// forever.
///
/// Inbound surfaces parse their own parameter into this enum; unknown strings are a 400 at the
/// gateway. Upstream renderers write the enum verbatim into the target's own level parameter.
/// There is no cross-vocabulary mapping, and there never will be — a value the target does not
/// support is rejected by the upstream and mirrored to the client per the existing error policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ReasoningLevel {
    /// No reasoning; renders as the native off spelling per format (`docs/reference/inference.md`).
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
    /// The one (level, wire-spelling) table of the closed vocabulary
    /// (`docs/reference/inference.md`), in [`Self::CANONICAL`] order. [`Self::as_str`] and
    /// [`Self::from_vocabulary_str`] are lookups into this table, and the format surfaces and
    /// the IR render from it, so a spelling change is a single edit here. The serde `lowercase`
    /// derive is the one wire spelling the table does not own — it renames the variant names
    /// independently — so a variant rename must update both the derive and this table.
    const VOCABULARY: [(ReasoningLevel, &'static str); Self::CANONICAL.len()] = [
        (Self::None, "none"),
        (Self::Minimal, "minimal"),
        (Self::Low, "low"),
        (Self::Medium, "medium"),
        (Self::High, "high"),
        (Self::XHigh, "xhigh"),
        (Self::Max, "max"),
    ];

    /// The vocabulary in canonical order — the storage and declaration order of a level set
    /// (`docs/design/storage.md`/`docs/design/providers.md`): a stored or declared set is
    /// deduplicated into this order at write.
    const CANONICAL: [Self; 7] = [
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
    /// foreign level, `docs/design/providers.md`), and the management override boundary (which
    /// rejects one) — each naming the vocabulary in its own error domain.
    #[must_use]
    pub(crate) fn from_vocabulary_str(value: &str) -> Option<Self> {
        Self::VOCABULARY
            .iter()
            .find(|(_, spelling)| *spelling == value)
            .map(|(level, _)| *level)
    }

    /// Canonicalizes a set of levels: deduplicated, in [`Self::CANONICAL`] order. Both write
    /// paths that store a level set — discovery ingestion and the management override — pass
    /// through here, so a stored set is always the canonical form (`docs/design/storage.md`).
    #[must_use]
    pub(crate) fn canonicalize_levels(levels: impl IntoIterator<Item = Self>) -> Vec<Self> {
        let levels: Vec<Self> = levels.into_iter().collect();

        Self::CANONICAL
            .into_iter()
            .filter(|canonical| levels.contains(canonical))
            .collect()
    }

    /// Renders the canonical vocabulary string, written verbatim into the target's own level
    /// parameter.
    ///
    /// # Panics
    ///
    /// Panics if a vocabulary member is missing from [`Self::VOCABULARY`] — impossible by
    /// construction: the table enumerates the whole closed vocabulary.
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        #[expect(
            clippy::unwrap_used,
            reason = "every vocabulary member is a row of the table by construction"
        )]
        Self::VOCABULARY
            .iter()
            .find(|(member, _)| *member == self)
            .map(|(_, spelling)| *spelling)
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the shared reasoning vocabulary: the closed string domain, canonical set
    //! ordering, and the spelling round-trip every format surface renders from
    //! (`docs/reference/inference.md`).

    #![expect(
        clippy::missing_panics_doc,
        reason = "a test fails by panicking; a # Panics section on every test would restate that"
    )]

    use super::ReasoningLevel;

    /// The vocabulary parses exactly its own seven spellings — case-sensitively, with no
    /// alternate separators — and every foreign string is refused.
    #[test]
    fn parsing_accepts_only_the_closed_spelling_domain() {
        let spellings = [
            ("none", ReasoningLevel::None),
            ("minimal", ReasoningLevel::Minimal),
            ("low", ReasoningLevel::Low),
            ("medium", ReasoningLevel::Medium),
            ("high", ReasoningLevel::High),
            ("xhigh", ReasoningLevel::XHigh),
            ("max", ReasoningLevel::Max),
        ];

        for (spelling, level) in spellings {
            assert_eq!(ReasoningLevel::from_vocabulary_str(spelling), Some(level));
            assert_eq!(level.as_str(), spelling);
            assert_eq!(
                ReasoningLevel::from_vocabulary_str(level.as_str()),
                Some(level)
            );
        }

        for foreign in ["", "LOW", "High", "x-high", "x_high", "ultra", "off"] {
            assert_eq!(ReasoningLevel::from_vocabulary_str(foreign), None);
        }
    }

    /// Set canonicalization deduplicates into the canonical declaration order regardless of the
    /// input's order or repetitions, and the empty set stays empty.
    #[test]
    fn canonicalization_deduplicates_into_canonical_order() {
        let canonical = ReasoningLevel::CANONICAL;

        assert_eq!(
            ReasoningLevel::canonicalize_levels([
                ReasoningLevel::High,
                ReasoningLevel::None,
                ReasoningLevel::High,
                ReasoningLevel::Max,
                ReasoningLevel::Low,
            ]),
            vec![
                ReasoningLevel::None,
                ReasoningLevel::Low,
                ReasoningLevel::High,
                ReasoningLevel::Max,
            ]
        );
        assert_eq!(
            ReasoningLevel::canonicalize_levels(canonical.into_iter().rev()),
            canonical.to_vec()
        );
        assert_eq!(
            ReasoningLevel::canonicalize_levels(std::iter::empty()),
            Vec::new()
        );
    }
}
