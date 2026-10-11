//! Routing-safe machine-consumed text: the one composition vocabulary every value the gateway
//! routes, addresses, or presents to an upstream satisfies (`docs/reference/management-api.md`).
//! The vocabulary is three predicates over one base contract: [`is_routing_safe_text`] — non-empty
//! printable ASCII without whitespace — which every routed value satisfies;
//! [`is_routing_safe_token`], which additionally bans `/` and `:`; and [`is_base_url`], which
//! additionally requires a non-empty `http://` or `https://` scheme.
//!
//! The boundary is the application layer alone — the management write boundary for operator
//! input, and the OAuth and discovery ingestion boundaries for upstream-reported values. The
//! database schema pins no composition `CHECK` constraints by design (`docs/design/storage.md`), so
//! these predicates are the single implementation every boundary shares; each boundary keeps only
//! its own error mapping.

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

/// Whether `value` is a base URL: routing-safe text that additionally carries a non-empty
/// `http://` or `https://` scheme followed by a non-empty rest; see
/// `docs/reference/management-api.md`.
/// The scheme match is case-sensitive.
#[must_use]
pub(crate) fn is_base_url(value: &str) -> bool {
    let after_scheme = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"));

    is_routing_safe_text(value) && after_scheme.is_some_and(|rest| !rest.is_empty())
}

#[cfg(test)]
mod tests {
    //! Tests for the routing-safe text composition vocabulary: the printable-ASCII base
    //! contract, the URL-path token bans, and the base-URL scheme requirement
    //! (`docs/reference/management-api.md`).

    #![expect(
        clippy::missing_panics_doc,
        reason = "a test fails by panicking; a # Panics section on every test would restate that"
    )]

    use super::{is_base_url, is_routing_safe_text, is_routing_safe_token};

    /// Routing-safe text is exactly non-empty printable ASCII: every printable ASCII character
    /// passes, while the empty string, ASCII whitespace, control characters, and non-ASCII text
    /// all fail.
    #[test]
    fn routing_safe_text_is_nonempty_printable_ascii_without_whitespace() {
        assert!(is_routing_safe_text("a"));
        assert!(is_routing_safe_text("gpt-4o:2024-08-06"));
        assert!(is_routing_safe_text("~!@#$%^&*()_+`[]{}|;'\",.<>?"));

        assert!(!is_routing_safe_text(""));
        assert!(!is_routing_safe_text(" "));
        assert!(!is_routing_safe_text("a b"));
        assert!(!is_routing_safe_text("a\tb"));
        assert!(!is_routing_safe_text("a\nb"));
        assert!(!is_routing_safe_text("\u{7f}"));
        assert!(!is_routing_safe_text("\u{1}"));
        assert!(!is_routing_safe_text("méga"));
        assert!(!is_routing_safe_text("模型"));
    }

    /// A routing token additionally bans exactly the two characters a Gemini URL path cannot
    /// address, `/` and `:`, while every other printable ASCII character stays acceptable.
    #[test]
    fn routing_safe_tokens_additionally_ban_slash_and_colon() {
        assert!(is_routing_safe_token("gemini-2.5-flash"));
        assert!(is_routing_safe_token("a+b_c~d"));

        assert!(!is_routing_safe_token("a/b"));
        assert!(!is_routing_safe_token("a:b"));
        assert!(!is_routing_safe_token(":"));
        assert!(!is_routing_safe_token("/"));
        assert!(!is_routing_safe_token(""));
        assert!(!is_routing_safe_token("a b"));
    }

    /// A base URL is routing-safe text with a case-sensitive `http://` or `https://` scheme and
    /// a non-empty rest: uppercase schemes, bare schemes, foreign schemes, and non-URL text all
    /// fail.
    #[test]
    fn base_urls_require_a_lowercase_http_scheme_and_a_rest() {
        assert!(is_base_url("https://api.example.com/v1"));
        assert!(is_base_url("http://localhost:8080"));
        assert!(is_base_url("http://a"));

        assert!(!is_base_url("https://"));
        assert!(!is_base_url("http://"));
        assert!(!is_base_url("HTTPS://api.example.com"));
        assert!(!is_base_url("Http://api.example.com"));
        assert!(!is_base_url("ftp://api.example.com"));
        assert!(!is_base_url("api.example.com"));
        assert!(!is_base_url("https://a b"));
        assert!(!is_base_url(""));
    }
}
