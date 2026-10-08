//! Routing-safe machine-consumed text: the one composition vocabulary every value the gateway
//! routes, addresses, or presents to an upstream satisfies (README §7). The vocabulary is three
//! predicates over one base contract: [`is_routing_safe_text`] — non-empty printable ASCII
//! without whitespace — which every routed value satisfies; [`is_routing_safe_token`], which
//! additionally bans `/` and `:`; and [`is_base_url`], which additionally requires a non-empty
//! `http://` or `https://` scheme.
//!
//! The boundary is the application layer alone — the management write boundary for operator
//! input, and the OAuth and discovery ingestion boundaries for upstream-reported values. The
//! database schema pins no composition `CHECK` constraints by design (README §3), so these
//! predicates are the single implementation every boundary shares; each boundary keeps only its
//! own error mapping.

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
/// `http://` or `https://` scheme — the scheme must be followed by a non-empty rest (README §7).
/// The scheme match is case-sensitive.
#[must_use]
pub(crate) fn is_base_url(value: &str) -> bool {
    let after_scheme = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"));

    is_routing_safe_text(value) && after_scheme.is_some_and(|rest| !rest.is_empty())
}
