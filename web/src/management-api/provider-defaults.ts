import type { ProviderType } from "/src/management-api/generated/v0";

/**
 * The name a fresh provider of each kind is suggested under: the account it
 * connects, so the operator names nothing in the common case. The two
 * OpenAI-compatible wire formats share one base; the numbered suggestion
 * below keeps them distinct.
 */
const DEFAULT_BASE_NAMES: Record<ProviderType, string> = {
    openai_response: "openai",
    openai_chat_completion: "openai",
    gemini_generate_content: "gemini",
    gemini_interaction: "gemini",
    anthropic_messages: "anthropic",
    codex_oauth: "codex",
    antigravity_oauth: "antigravity",
};

/** The suggested base name for a fresh provider of this kind. */
export function defaultProviderBaseName(kind: ProviderType): string {
    return DEFAULT_BASE_NAMES[kind];
}

/**
 * The first free name for a fresh provider: the kind's base name, then the
 * base with `-1`, `-2`, and so on while taken names collide. Gaps left by a
 * renamed provider are reused from the lowest suffix up.
 */
export function suggestProviderName(kind: ProviderType, takenNames: ReadonlySet<string>): string {
    const base = defaultProviderBaseName(kind);

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
 * name to derive from. A slash is never used: the Gemini URL path cannot
 * address `/` or `:` (gateway README §7).
 */
export function defaultModelPrefix(name: string): string {
    return name === "" ? "" : `${name}-`;
}
