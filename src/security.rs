//! Constant-time secret comparison helpers.

/// Compares two strings for equality without branching on their contents.
///
/// The comparison folds the XOR of every byte pair and of both lengths into a
/// single accumulator, so content mismatches never short-circuit. Length is
/// intentionally folded into the accumulator rather than kept secret: token
/// lengths are not confidential, and a length mismatch can never leak partial
/// content.
pub(crate) fn constant_time_eq(candidate: &str, expected: &str) -> bool {
    let candidate_bytes = candidate.as_bytes();
    let expected_bytes = expected.as_bytes();

    let mut difference = candidate_bytes.len() ^ expected_bytes.len();

    for (candidate_byte, expected_byte) in candidate_bytes.iter().zip(expected_bytes) {
        difference |= usize::from(candidate_byte ^ expected_byte);
    }

    difference == 0
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq as eq;

    /// Verifies that matches equal strings holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn matches_equal_strings() {
        assert!(eq("secret-token", "secret-token"));
        assert!(eq("", ""));
    }

    /// Verifies that rejects mismatched content holds.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn rejects_mismatched_content() {
        assert!(!eq("secret-token", "secret-tokem"));
        assert!(!eq("secret-token", "secret"));
        assert!(!eq("", "secret"));
        assert!(!eq("secret", ""));
    }
}
