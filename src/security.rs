//! Constant-time secret comparison helpers.

use subtle::ConstantTimeEq as _;

/// Compares two strings for equality without branching on their contents.
///
/// Delegates to `subtle`'s audited [`ConstantTimeEq`](subtle::ConstantTimeEq) implementation over
/// the raw bytes; a content
/// mismatch never short-circuits the comparison.
pub(crate) fn constant_time_eq(candidate: &str, expected: &str) -> bool {
    candidate.as_bytes().ct_eq(expected.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    //! Tests for the constant-time comparison wrapper: byte-exact equality over strings,
    //! including mismatched lengths and empty inputs.

    #![expect(
        clippy::missing_panics_doc,
        reason = "a test fails by panicking; a # Panics section on every test would restate that"
    )]

    use super::constant_time_eq;

    /// Comparison is byte-exact equality over the whole string: any content difference, any
    /// length difference, and the empty-on-empty case all classify without ambiguity.
    #[test]
    fn comparison_is_byte_exact_equality() {
        assert!(constant_time_eq("serving-key", "serving-key"));
        assert!(!constant_time_eq("serving-key", "serving-keY"));
        assert!(!constant_time_eq("serving-key", "serving-ke"));
        assert!(!constant_time_eq("serving-ke", "serving-key"));
        assert!(!constant_time_eq("", "serving-key"));
        assert!(constant_time_eq("", ""));
    }
}
