/**
 * Composition rules for machine-consumed management fields (gateway README
 * §7): routing identifiers, model prefixes, bearer secrets, and base URLs.
 * Every predicate mirrors the gateway's own validation and its schema
 * `CHECK` constraints, so the app rejects exactly what the gateway would
 * reject — before the round trip, with the field named.
 *
 * The shared rule is printable ASCII without whitespace (`U+0021–U+007E`):
 * no spaces, no control characters, no non-ASCII text such as CJK. Routing
 * tokens — upstream IDs, present overrides, and model prefixes — additionally
 * reject `/` and `:` (the Gemini URL path cannot address them). Human
 * labels (provider and serving-key names) stay free-form and are not
 * covered here.
 */

/** Non-empty printable ASCII without whitespace. */
const COMPOSITION_PATTERN = /^[!-~]+$/u;

/** Printable ASCII without whitespace, empty included. */
const COMPOSITION_OR_EMPTY_PATTERN = /^[!-~]*$/u;

/**
 * Whether the value is a routing token: an upstream ID or a present override —
 * non-empty printable ASCII without whitespace and without `/` or `:` (the Gemini
 * URL path cannot address them).
 */
export function isRoutingToken(value: string): boolean {
    return COMPOSITION_PATTERN.test(value) && !value.includes("/") && !value.includes(":");
}

/**
 * Whether the value is a valid model prefix: empty means no prefix,
 * otherwise the routing-token composition — printable ASCII without whitespace
 * and without `/` or `:`.
 */
export function isModelPrefix(value: string): boolean {
    return COMPOSITION_OR_EMPTY_PATTERN.test(value) && !value.includes("/") && !value.includes(":");
}

/**
 * Whether the value is a valid bearer secret: a serving-key secret, an
 * upstream static key, or a hand-replaced OAuth credential field — non-empty
 * printable ASCII without whitespace, so the secret always renders into its
 * header verbatim.
 */
export function isBearerSecret(value: string): boolean {
    return COMPOSITION_PATTERN.test(value);
}

/**
 * Whether the value is a valid static-key base URL: a non-empty `http://`
 * or `https://` address of printable ASCII without whitespace.
 */
export function isBaseUrl(value: string): boolean {
    const rest = value.startsWith("http://")
        ? value.slice("http://".length)
        : value.startsWith("https://")
          ? value.slice("https://".length)
          : null;

    return rest !== null && rest !== "" && COMPOSITION_PATTERN.test(value);
}

/**
 * Human wording for a composition rejection of the named field, shown where
 * the value was typed.
 */
export function compositionMessage(field: string): string {
    return `${field} accepts letters, numbers, and symbols only — no spaces or non-ASCII text.`;
}

/** Human wording for a routing-token rejection, shown where the value was typed. */
export function routingTokenMessage(field: string): string {
    return `${field} accepts letters, numbers, and symbols only — no spaces, non-ASCII text, "/", or ":".`;
}

/** Human wording for a base URL rejection, shown where the URL was typed. */
export function baseUrlMessage(): string {
    return "Base URL must be an http:// or https:// address with no spaces.";
}
