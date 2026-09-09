//! Constant-time secret comparison helpers.

use subtle::ConstantTimeEq as _;

/// Compares two strings for equality without branching on their contents.
///
/// Delegates to `subtle`'s audited [`ConstantTimeEq`] implementation over the
/// raw bytes; a content mismatch never short-circuits the comparison.
pub(crate) fn constant_time_eq(candidate: &str, expected: &str) -> bool {
    candidate.as_bytes().ct_eq(expected.as_bytes()).into()
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
