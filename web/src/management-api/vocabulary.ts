import { ContractViolationError } from "/src/management-api/errors";

import {
    type OAuthProviderType,
    type ProviderResponse,
    type ProviderType,
    THINKING_LEVEL_ORDER,
    type ThinkingLevel,
} from "/src/management-api/generated/v0";

export type VocabularyOption<TValue extends string> = {
    readonly value: TValue;
    readonly label: string;
};

/**
 * Every provider kind with the label each is presented under. The record type
 * is exhaustive, so a kind added to the generated union fails to compile
 * until it is listed here.
 */
const PROVIDER_TYPE_LABELS: Record<ProviderType, string> = {
    openai_response: "OpenAI Responses API",
    openai_chat_completion: "OpenAI Chat Completions API",
    gemini_generate_content: "Gemini generateContent API",
    codex_oauth: "Codex OAuth",
    antigravity_oauth: "Antigravity OAuth",
};

/**
 * The same vocabulary as options. The record type makes the key set exactly the
 * union, so the enumerated keys are members of it by construction.
 */
export const PROVIDER_TYPE_OPTIONS: ReadonlyArray<VocabularyOption<ProviderType>> = Object.entries(
    PROVIDER_TYPE_LABELS,
).map(([value, label]) => ({
    value: value as ProviderType,
    label,
}));

const OAUTH_PROVIDER_TYPE_LABELS: Record<OAuthProviderType, string> = {
    codex_oauth: "Codex OAuth",
    antigravity_oauth: "Antigravity OAuth",
};

/** The OAuth provider kinds as options; the record type is exhaustive as above. */
export const OAUTH_PROVIDER_TYPE_OPTIONS: ReadonlyArray<VocabularyOption<OAuthProviderType>> =
    Object.entries(OAUTH_PROVIDER_TYPE_LABELS).map(([value, label]) => ({
        value: value as OAuthProviderType,
        label,
    }));

/** The label a provider row's own `type` is presented under. */
export function providerTypeLabel(type: ProviderResponse["type"]): string {
    return PROVIDER_TYPE_LABELS[type];
}

/** Looks a value up in a vocabulary, returning `null` when it is not a member. */
function findOption<TValue extends string>(
    options: ReadonlyArray<VocabularyOption<TValue>>,
    value: unknown,
): TValue | null {
    for (const option of options) {
        if (option.value === value) {
            return option.value;
        }
    }

    return null;
}

export function findOAuthProviderType(value: unknown): OAuthProviderType | null {
    return findOption(OAUTH_PROVIDER_TYPE_OPTIONS, value);
}

/**
 * Whether a stored row is an OAuth login: exactly the variants carrying
 * `expires_at`. Narrows the row, so the expiry is readable past the guard.
 */
export function isOAuthProviderRow(provider: ProviderResponse): provider is Extract<
    ProviderResponse,
    {
        type: OAuthProviderType;
    }
> {
    return findOAuthProviderType(provider.type) !== null;
}

export function findThinkingLevel(value: unknown): ThinkingLevel | null {
    const text = String(value);

    for (const level of THINKING_LEVEL_ORDER) {
        if (level === text) {
            return level;
        }
    }

    return null;
}

/** Narrows a wire value that must be one of the canonical thinking levels. */
export function parseThinkingLevel(value: unknown, context: string): ThinkingLevel {
    const level = findThinkingLevel(value);

    if (level === null) {
        throw new ContractViolationError(
            `${context} is not a canonical thinking level: ${String(value)}`,
        );
    }

    return level;
}
