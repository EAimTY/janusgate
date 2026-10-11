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
    const fn as_str(self) -> &'static str {
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
    source_id: Option<String>,

    /// Deterministic lowercase-hex digest of the typed semantic item plus its
    /// occurrence position.
    digest: String,
}

impl ItemIdentity {
    /// Computes the identity of one instruction item: its role, its occurrence
    /// index among instructions of the same role within the compared window,
    /// its text, and its client source identifier when available. An empty
    /// identifier normalizes to absent: a built identity never carries one, so
    /// the non-empty source identifiers [`Self::matches`] matches are enforced
    /// at construction.
    #[must_use]
    fn for_instruction(
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
/// digest form every carrier comparison uses (`docs/design/compaction.md`).
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
fn hex_digest(bytes: &[u8]) -> String {
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
        clippy::missing_panics_doc,
        reason = "a test fails by panicking on its own assertions; a # Panics section on every \
                  test would document nothing its name does not"
    )]

    use super::{InstructionRole, ItemIdentity, hex_sha256};
    use std::error::Error as StdError;

    /// The occurrence index counts only items of the instruction's own role, so interleaving
    /// another role's items never changes an item's identity: the developer identity is stable
    /// whether system items precede it or not, while a second same-role item digests differently
    /// from the first.
    #[test]
    fn occurrence_indices_are_per_role() -> Result<(), Box<dyn StdError>> {
        let interleaved = ItemIdentity::for_instruction_window([
            (InstructionRole::System, "system one"),
            (InstructionRole::Developer, "developer rule"),
            (InstructionRole::System, "system two"),
        ]);

        let plain = ItemIdentity::for_instruction_window([
            (InstructionRole::Developer, "developer rule"),
            (InstructionRole::System, "system two"),
        ]);

        let [system_one, developer_interleaved, system_two] = interleaved.as_slice() else {
            return Err("the interleaved window yields exactly three identities".into());
        };

        let [developer_plain, _] = plain.as_slice() else {
            return Err("the plain window yields exactly two identities".into());
        };

        // The developer item is the role's first occurrence in both windows: interleaving system
        // items leaves its digest untouched.
        assert!(developer_interleaved.matches(developer_plain));

        // The second system item is that role's second occurrence, distinct from the first even
        // though every other window fact is shared.
        assert!(!system_two.matches(system_one));

        Ok(())
    }

    /// Two identities match on equal non-empty source identifiers regardless of their digests, on
    /// equal digests when the identifiers differ, and never on a text-similarity guess.
    #[test]
    fn matching_prefers_source_id_and_falls_back_to_digest() {
        let digest_of =
            |role, occurrence, text| ItemIdentity::for_instruction(role, occurrence, text, None);

        let sourced = ItemIdentity::for_instruction(
            InstructionRole::System,
            0,
            "current text",
            Some(String::from("src-1")),
        );

        let other_sourced = ItemIdentity::for_instruction(
            InstructionRole::System,
            0,
            "covered text",
            Some(String::from("src-1")),
        );

        assert!(
            sourced.matches(&other_sourced),
            "equal source identifiers match across different texts"
        );

        let conflicting_sourced = ItemIdentity::for_instruction(
            InstructionRole::System,
            0,
            "covered text",
            Some(String::from("src-2")),
        );

        assert!(
            !sourced.matches(&conflicting_sourced),
            "different source identifiers and different digests never match"
        );

        let same_text_unsourced = digest_of(InstructionRole::System, 0, "current text");

        assert!(
            sourced.matches(&same_text_unsourced),
            "an equal digest matches even when only one side carries a source identifier"
        );

        let covered_unsourced = digest_of(InstructionRole::System, 0, "covered text");

        assert!(
            !sourced.matches(&covered_unsourced),
            "a source-carrying side never falls to a text-similarity match"
        );
    }

    /// An empty source identifier normalizes to absent at construction, so an empty-identifier
    /// identity behaves exactly like an unsourced one: it matches only on digest.
    #[test]
    fn empty_source_id_normalizes_to_absent() {
        let empty_sourced = ItemIdentity::for_instruction(
            InstructionRole::Developer,
            0,
            "same text",
            Some(String::new()),
        );

        let unsourced =
            ItemIdentity::for_instruction(InstructionRole::Developer, 0, "same text", None);

        assert!(empty_sourced.matches(&unsourced));

        let other_text =
            ItemIdentity::for_instruction(InstructionRole::Developer, 0, "other text", None);

        assert!(
            !empty_sourced.matches(&other_text),
            "a normalized-absent identifier matches on digest alone"
        );
    }

    /// The digest rendering is the documented lowercase hex SHA-256: the empty input's known
    /// digest pins the spelling, and a changed input changes the digest.
    #[test]
    fn hex_sha256_renders_lowercase_hex() {
        assert_eq!(
            hex_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        assert_eq!(
            hex_sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
