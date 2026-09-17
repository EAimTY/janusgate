//! Constant-time secret comparison helpers.

use subtle::ConstantTimeEq as _;

/// Compares two strings for equality without branching on their contents.
///
/// Delegates to `subtle`'s audited [`ConstantTimeEq`] implementation over the raw bytes; a content
/// mismatch never short-circuits the comparison.
pub(crate) fn constant_time_eq(candidate: &str, expected: &str) -> bool {
    candidate.as_bytes().ct_eq(expected.as_bytes()).into()
}
