//! Stable item identities for overlap detection across a compaction boundary.
//!
//! A checkpoint's envelope records the identities of the instruction items and
//! retained units it covers. On replay, history entries sitting before the
//! boundary are compared against those identities: proven overlap is removed
//! from the covered window — the envelope restores it — while an unmatched
//! entry (for example, a developer instruction a client newly injected before
//! the checkpoint) is preserved. Comparison is ordered identity overlap, never
//! arbitrary text similarity, and never a global deduplication of equal text.
//!
//! An identity is a stable client-supplied source identifier when one is
//! available, otherwise a deterministic digest of the typed semantic item plus
//! its occurrence position: the index of the entry among entries of the same
//! kind and role within the compared window. Both sides of the comparison
//! compute over their own prefix, so an unchanged prefix yields identical
//! digests.

use crate::ir::{IrMessage, IrRole};
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

/// The stable identity of one instruction item or retained unit: its client
/// source identifier when one is available, and its deterministic digest.
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
    /// its text, and its client source identifier when available.
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
            source_id,
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
        // Per-role occurrence counters over the compared window.
        let mut occurrences: Vec<(InstructionRole, usize)> = Vec::new();

        items
            .into_iter()
            .map(|(role, text)| {
                let occurrence = occurrences
                    .iter()
                    .find(|(counted, _)| *counted == role)
                    .map(|(_, count)| *count)
                    .unwrap_or(0);

                if let Some((_, count)) =
                    occurrences.iter_mut().find(|(counted, _)| *counted == role)
                {
                    *count += 1;
                } else {
                    occurrences.push((role, 1));
                }

                Self::for_instruction(role, occurrence, text, None)
            })
            .collect()
    }

    /// Computes the identity of one conversation message: its occurrence index
    /// among messages of the message's own role within the compared window,
    /// its typed semantic content, and its client source identifier when
    /// available.
    ///
    /// # Panics
    ///
    /// Panics if the typed semantic serialization of the message fails —
    /// impossible for IR parts over concrete serde types.
    #[must_use]
    pub(crate) fn for_message(
        occurrence: usize,
        message: &IrMessage,
        source_id: Option<String>,
    ) -> Self {
        // The digest covers the message's typed semantic serialization, which
        // both the sealing side (the covered window) and the replay side (the
        // current request's prefix) recompute from their own parsed history.
        #[expect(
            clippy::expect_used,
            reason = "serializing concrete serde types over IR parts cannot fail"
        )]
        let semantic = serde_json::to_vec(&MessageDigest {
            role: message.role,
            parts: &message.parts,
        })
        .expect("the typed semantic item serializes");

        let mut hasher = Sha256::new();

        hasher.update(b"message\x00");
        hasher.update(role_spelling(message.role).as_bytes());
        hasher.update(b"\x00");
        hasher.update(occurrence.to_string().as_bytes());
        hasher.update(b"\x00");
        hasher.update(&semantic);

        Self {
            source_id,
            digest: hex_digest(&hasher.finalize()),
        }
    }

    /// Whether this identity matches another: equal non-empty source
    /// identifiers, or equal digests.
    #[must_use]
    pub(crate) fn matches(&self, other: &Self) -> bool {
        match (&self.source_id, &other.source_id) {
            (Some(left), Some(right)) => left == right || self.digest == other.digest,

            _ => self.digest == other.digest,
        }
    }
}

/// The semantic digest input of a conversation message: role plus parts, in a
/// private serializable shape so the digest is stable regardless of unrelated
/// `IrMessage` changes.
#[derive(Serialize)]
struct MessageDigest<'a> {
    /// The message's participating role.
    role: IrRole,

    /// The message's ordered typed segments.
    parts: &'a [crate::ir::IrPart],
}

/// Renders a role's digest spelling.
const fn role_spelling(role: IrRole) -> &'static str {
    match role {
        IrRole::User => "user",
        IrRole::Assistant => "assistant",
        IrRole::Tool => "tool",
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
#[must_use]
pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        rendered.push_str(&format!("{byte:02x}"));
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
    use crate::ir::{IrMessage, IrPart, IrRole};

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

    /// Message digests are role-sensitive and content-sensitive.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn message_identities_distinguish_role_and_content() {
        let user = IrMessage {
            role: IrRole::User,
            parts: vec![IrPart::Text {
                text: String::from("hello"),
            }],
        };

        let assistant = IrMessage {
            role: IrRole::Assistant,
            parts: vec![IrPart::Text {
                text: String::from("hello"),
            }],
        };

        let left = ItemIdentity::for_message(0, &user, None);
        let right = ItemIdentity::for_message(0, &user, None);

        assert!(left.matches(&right));
        assert!(!left.matches(&ItemIdentity::for_message(0, &assistant, None)));
        assert!(!left.matches(&ItemIdentity::for_message(1, &user, None)));
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
