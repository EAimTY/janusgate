//! Routing-safe machine-consumed text: the one composition vocabulary every value the gateway
//! routes, addresses, or presents to an upstream satisfies (README §7).
//!
//! The boundary is the application layer alone — the management write boundary for operator
//! input, and the OAuth and discovery ingestion boundaries for upstream-reported values. The
//! database schema pins no composition `CHECK` constraints by design (README §3), so these
//! predicates are the single implementation every boundary shares.

/// Whether `value` is routing-safe text: non-empty printable ASCII without whitespace
/// (`U+0021`–`U+007E`) — no spaces, no control characters, no non-ASCII text such as CJK.
#[must_use]
pub(crate) fn is_routing_safe_text(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_graphic())
}

/// Whether `value` is a routing token: routing-safe text that additionally carries neither `/`
/// nor `:` — the Gemini URL path (`/models/{model}:generateContent`) cannot address either.
#[must_use]
pub(crate) fn is_routing_safe_token(value: &str) -> bool {
    is_routing_safe_text(value) && !value.contains('/') && !value.contains(':')
}

#[cfg(test)]
mod tests {
    use super::{is_routing_safe_text, is_routing_safe_token};

    /// Pins the two shared composition predicates (README §7): routing-safe text accepts
    /// printable ASCII only, and the routing-token variant additionally bans `/` and `:`.
    ///
    /// # Panics
    ///
    /// Panics if a value classifies outside its documented set.
    #[test]
    fn classifies_routing_safe_text_and_tokens() {
        let text_cases = [
            ("gpt-4o", true),
            ("custom-model_1.2", true),
            ("https://upstream.example/v1", true),
            ("", false),
            ("has space", false),
            ("tab\tstopped", false),
            ("日本語", false),
            ("control\u{7f}", false),
        ];

        for (value, is_valid) in text_cases {
            assert_eq!(
                is_routing_safe_text(value),
                is_valid,
                "text classification mismatch for `{value}`"
            );
        }

        let token_cases = [
            ("gpt-4o", true),
            ("slashed/model", false),
            ("coloned:model", false),
        ];

        for (value, is_valid) in token_cases {
            assert_eq!(
                is_routing_safe_token(value),
                is_valid,
                "token classification mismatch for `{value}`"
            );
        }

        // A routing token is never broader than routing-safe text.
        assert!(!is_routing_safe_token("has space"));
    }
}
