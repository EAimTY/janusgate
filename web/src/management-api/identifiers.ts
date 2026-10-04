import { translation } from "/src/i18n";

/**
 * Composition rules for machine-consumed management fields (gateway README
 * §7): routing identifiers, bearer secrets, and base URLs.
 * Every predicate mirrors the gateway's boundary validation alone, so the
 * app rejects exactly what the gateway would reject — before the round
 * trip, with the field named.
 *
 * The shared rule is printable ASCII without whitespace (`U+0021–U+007E`):
 * no spaces, no control characters, no non-ASCII text such as CJK. Routing
 * tokens — upstream IDs and public route IDs — additionally
 * reject `/` and `:` (the Gemini URL path cannot address them). Human
 * labels (provider and serving-key names) stay free-form and are not
 * covered here.
 */

/** Non-empty printable ASCII without whitespace. */
const COMPOSITION_PATTERN = /^[!-~]+$/u;

/**
 * Whether the value is a routing token: an upstream ID or a public route ID —
 * non-empty printable ASCII without whitespace and without `/` or `:` (the Gemini
 * URL path cannot address them).
 */
export function isRoutingToken(value: string): boolean {
    return COMPOSITION_PATTERN.test(value) && !value.includes("/") && !value.includes(":");
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
export type CompositionField = "upstreamId" | "publicId" | "modelPrefix" | "apiKey" | "key";

export function compositionMessage(field: CompositionField): string {
    return translation()(($) => $.validation.composition[field]);
}

/** Human wording for a routing-token rejection, shown where the value was typed. */
export function routingTokenMessage(field: CompositionField): string {
    return translation()(($) => $.validation.routing[field]);
}

/** Human wording for a base URL rejection, shown where the URL was typed. */
export function baseUrlMessage(): string {
    return translation()(
        ($) => $.helpers.identifiers.baseURLMustBeAnHttpOrHttpsAddressWithNoSpaces,
    );
}

/**
 * The inline error for a bearer secret — a serving-key secret, an upstream
 * static key, or a hand-replaced OAuth credential field — shown where it was
 * typed. An empty value is allowed: the write-only field's absence keeps the
 * stored secret, so only a typed value is judged.
 */
export function bearerSecretError(value: string, label: CompositionField): string | null {
    return value === "" || isBearerSecret(value) ? null : compositionMessage(label);
}

/**
 * The inline error for an upstream or public route ID. Empty input is the
 * form's not-yet-typed state, named by the submit gate instead.
 */
export function routingTokenError(value: string | null, label: CompositionField): string | null {
    return value === null || value === "" || isRoutingToken(value)
        ? null
        : routingTokenMessage(label);
}
