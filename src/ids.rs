//! UUID generation for gateway-issued identifiers.

use uuid::Uuid;

/// Generates a random UUID v4 in its canonical lowercase hyphenated form.
pub(crate) fn new_uuid() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    //! Tests for the gateway-issued identifier shape: the canonical lowercase hyphenated UUID
    //! v4 form and per-call randomness.

    #![expect(
        clippy::missing_panics_doc,
        reason = "a test fails by panicking; a # Panics section on every test would restate that"
    )]

    use super::new_uuid;

    /// Every generated identifier is 36 characters of lowercase hex in the canonical hyphenated
    /// layout, carrying the version-4 and RFC 9562 variant nibbles, and successive calls never
    /// collide.
    #[test]
    fn generated_identifiers_are_canonical_v4_forms() {
        let first = new_uuid();
        let second = new_uuid();

        assert_eq!(first.len(), 36);
        assert_ne!(first, second);

        let hyphens: Vec<usize> = first
            .char_indices()
            .filter(|(_, character)| *character == '-')
            .map(|(index, _)| index)
            .collect();

        assert_eq!(hyphens, vec![8, 13, 18, 23]);

        let hex: String = first
            .chars()
            .filter(|character| *character != '-')
            .collect();

        assert!(
            hex.chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
        );
        assert_eq!(hex.chars().nth(12), Some('4'));
        assert!(matches!(hex.chars().nth(16), Some('8' | '9' | 'a' | 'b')));
    }
}
