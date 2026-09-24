import type { UpstreamFormat } from "/src/management-api/generated/v0";
import type { CredentialKind } from "/src/management-api/vocabulary";

/**
 * The name a fresh provider of each wire format is suggested under: the
 * product family it connects, so the operator names nothing in the common
 * case. The preset that prefilled the form suggests its own name first; this
 * map is the fallback for a format chosen without a preset.
 */
const DEFAULT_BASE_NAMES: Record<UpstreamFormat, string> = {
    openai_response: "openai",
    openai_chat_completion: "openai",
    gemini_generate_content: "gemini",
    gemini_interaction: "gemini",
    anthropic_messages: "anthropic",
};

/** The suggested base name for a fresh provider of this format. */
export function defaultProviderBaseName(format: UpstreamFormat): string {
    return DEFAULT_BASE_NAMES[format];
}

/** The suggested base name for a fresh provider of this credential kind. */
export function defaultKindBaseName(kind: CredentialKind): string {
    return kind === "oauth" ? "oauth" : "static";
}

/**
 * The first free name for a fresh provider: the kind's base name, then the
 * base with `-1`, `-2`, and so on while taken names collide. Gaps left by a
 * renamed provider are reused from the lowest suffix up.
 */
export function suggestProviderName(base: string, takenNames: ReadonlySet<string>): string {
    if (!takenNames.has(base)) {
        return base;
    }

    let suffix = 1;

    while (takenNames.has(`${base}-${suffix}`)) {
        suffix += 1;
    }

    return `${base}-${suffix}`;
}

/**
 * The default model prefix: the provider's name with one trailing dash, so
 * an unoverridden model reads as `name-upstream-id`. Empty while there is no
 * name to derive from. A slash is never used: it is a routing token, and the
 * Gemini URL path cannot address one.
 */
export function defaultModelPrefix(name: string): string {
    return name === "" ? "" : `${name}-`;
}
