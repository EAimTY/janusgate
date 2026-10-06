//! Stable item identities for overlap detection across a compaction boundary.
//!
//! A checkpoint's envelope records the identities of the instruction items it
//! covers. On replay, instruction entries sitting before the boundary are
//! compared against those identities: proven overlap is removed from the
//! covered window — the envelope restores it — while an unmatched entry (for
//! example, a developer instruction a client newly injected before the
//! checkpoint) is preserved. The conversation prefix supersedes wholesale at
//! the boundary offset and is never matched by identity. Comparison is ordered
//! identity overlap, never arbitrary text similarity, and never a global
//! deduplication of equal text.
//!
//! An identity is a stable client-supplied source identifier when one is
//! available, otherwise a deterministic digest of the typed semantic item plus
//! its occurrence position: the index of the entry among entries of the same
//! role within the compared window. Both sides of the comparison compute over
//! their own prefix, so an unchanged prefix yields identical digests.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Role of a preserved instruction item, kept separate from conversation
/// content (past conversation must never be promoted into instruction
/// authority).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InstructionRole {
    /// A system instruction.
    System,

    /// A developer instruction.
    Developer,
}

impl InstructionRole {
    /// The role's canonical spelling.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Developer => "developer",
        }
    }
}

/// The stable identity of one instruction item: its client source identifier
/// when one is available, and its deterministic digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ItemIdentity {
    /// The client-supplied source identifier, when the source protocol carries
    /// one.
    pub(crate) source_id: Option<String>,

    /// Deterministic lowercase-hex digest of the typed semantic item plus its
    /// occurrence position.
    pub(crate) digest: String,
}

impl ItemIdentity {
    /// Computes the identity of one instruction item: its role, its occurrence
    /// index among instructions of the same role within the compared window,
    /// its text, and its client source identifier when available. An empty
    /// identifier normalizes to absent: a built identity never carries one, so
    /// the non-empty source identifiers [`Self::matches`] matches are enforced
    /// at construction.
    #[must_use]
    pub(crate) fn for_instruction(
        role: InstructionRole,
        occurrence: usize,
        text: &str,
        source_id: Option<String>,
    ) -> Self {
        let mut hasher = Sha256::new();

        hasher.update(b"instruction\x00");
        hasher.update(role.as_str().as_bytes());
        hasher.update(b"\x00");
        hasher.update(occurrence.to_string().as_bytes());
        hasher.update(b"\x00");
        hasher.update(text.as_bytes());

        Self {
            source_id: source_id.filter(|identifier| !identifier.is_empty()),
            digest: hex_digest(&hasher.finalize()),
        }
    }

    /// Computes the identities of a window of instruction items in order:
    /// each item's occurrence index counts only the items of its own role
    /// within the window, so an unchanged same-role prefix yields identical
    /// digests on both sides of a replay comparison.
    #[must_use]
    pub(crate) fn for_instruction_window<'a>(
        items: impl IntoIterator<Item = (InstructionRole, &'a str)>,
    ) -> Vec<Self> {
        // One occurrence counter per role: the instruction vocabulary is
        // closed, so two counters index any window in a single pass.
        let mut system_occurrences = 0usize;
        let mut developer_occurrences = 0usize;

        items
            .into_iter()
            .map(|(role, text)| {
                let occurrence = match role {
                    InstructionRole::System => {
                        let occurrence = system_occurrences;
                        system_occurrences += 1;
                        occurrence
                    }

                    InstructionRole::Developer => {
                        let occurrence = developer_occurrences;
                        developer_occurrences += 1;
                        occurrence
                    }
                };

                Self::for_instruction(role, occurrence, text, None)
            })
            .collect()
    }

    /// Whether this identity matches another: equal non-empty source
    /// identifiers, or equal digests. Built identities never carry an empty
    /// source identifier — the constructors normalize one to absent — so the
    /// non-empty half of the contract holds by construction.
    #[must_use]
    pub(crate) fn matches(&self, other: &Self) -> bool {
        match (&self.source_id, &other.source_id) {
            (Some(left), Some(right)) => left == right || self.digest == other.digest,

            _ => self.digest == other.digest,
        }
    }
}

/// Renders the SHA-256 of `bytes` as lowercase hex — the binding and identity
/// digest form every carrier comparison uses (README §6.8).
#[must_use]
pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    hex_digest(&Sha256::digest(bytes))
}

/// Renders a SHA-256 digest as lowercase hex — the digest form every
/// identity, visible-content binding, and prefix requirement carries.
///
/// # Panics
///
/// Panics if the fmt write into the rendering `String` fails — impossible, a
/// `String`'s fmt buffer is always writable.
#[must_use]
pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        #[expect(
            clippy::expect_used,
            reason = "writing to a String's fmt buffer cannot fail"
        )]
        std::fmt::Write::write_fmt(&mut rendered, format_args!("{byte:02x}"))
            .expect("a String's fmt buffer is always writable");
    }

    rendered
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "fixtures index values they constructed themselves"
    )]

    use super::{InstructionRole, ItemIdentity};

    /// Identities computed over equal semantic items in equal positions match,
    /// and a source identifier wins over a digest when both sides carry one.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn identities_match_on_equal_semantics_and_source_ids() {
        let left = ItemIdentity::for_instruction(InstructionRole::System, 0, "be terse", None);
        let right = ItemIdentity::for_instruction(InstructionRole::System, 0, "be terse", None);

        assert!(left.matches(&right));

        let shifted = ItemIdentity::for_instruction(InstructionRole::System, 1, "be terse", None);

        assert!(
            !left.matches(&shifted),
            "a shifted occurrence is not overlap"
        );

        let sourced = ItemIdentity::for_instruction(
            InstructionRole::System,
            0,
            "be terse",
            Some(String::from("id-7")),
        );
        let sourced_again = ItemIdentity::for_instruction(
            InstructionRole::System,
            2,
            "elsewhere",
            Some(String::from("id-7")),
        );

        assert!(
            sourced.matches(&sourced_again),
            "equal source identifiers match across positions"
        );

        let other_sourced = ItemIdentity::for_instruction(
            InstructionRole::System,
            0,
            "be verbose",
            Some(String::from("id-8")),
        );

        assert!(
            !sourced.matches(&other_sourced),
            "differing source identifiers fall back to the digest"
        );
    }

    /// An empty source identifier normalizes to absent at construction, so a
    /// built identity never carries the empty identifier `matches` excludes.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn empty_source_identifiers_normalize_to_absent() {
        let identity = ItemIdentity::for_instruction(
            InstructionRole::System,
            0,
            "be terse",
            Some(String::new()),
        );

        assert_eq!(identity.source_id, None);
    }

    /// The window identities count occurrences per role, so interleaved roles
    /// each index from zero.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn window_identities_count_occurrences_per_role() {
        let system = InstructionRole::System;
        let developer = InstructionRole::Developer;

        let identities = ItemIdentity::for_instruction_window([
            (system, "first system"),
            (developer, "first developer"),
            (system, "second system"),
        ]);

        assert!(identities[0].matches(&ItemIdentity::for_instruction(
            system,
            0,
            "first system",
            None
        )));
        assert!(identities[1].matches(&ItemIdentity::for_instruction(
            developer,
            0,
            "first developer",
            None
        )));
        assert!(identities[2].matches(&ItemIdentity::for_instruction(
            system,
            1,
            "second system",
            None
        )));
    }
}
