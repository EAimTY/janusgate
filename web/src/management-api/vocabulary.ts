import { ContractViolationError } from "/src/management-api/errors";
import {
    type OAuthProviderType,
    type ProviderResponse,
    THINKING_LEVEL_ORDER,
    type ThinkingLevel,
    type UpstreamFormat,
} from "/src/management-api/generated/v0";

export type VocabularyOption<TValue extends string> = {
    readonly value: TValue;
    readonly label: string;
};

/**
 * The upstream wire formats with the label each is presented under. The record
 * type is exhaustive, so a variant added to the generated union fails to
 * compile until it is listed here.
 */
const UPSTREAM_FORMAT_LABELS: Record<UpstreamFormat, string> = {
    openai_response: "OpenAI Responses API",
    openai_chat_completion: "OpenAI Chat Completions API",
    gemini_generate_content: "Gemini generateContent API",
};

/**
 * The same vocabulary as options. The record type makes the key set exactly the
 * union, so the enumerated keys are members of it by construction.
 */
export const UPSTREAM_FORMAT_OPTIONS: ReadonlyArray<VocabularyOption<UpstreamFormat>> =
    Object.entries(UPSTREAM_FORMAT_LABELS).map(([value, label]) => ({
        value: value as UpstreamFormat,
        label,
    }));

const OAUTH_PROVIDER_TYPE_LABELS: Record<OAuthProviderType, string> = {
    openai_oauth: "OpenAI (ChatGPT account)",
    antigravity_oauth: "Google Antigravity",
};

const OPENAI_COMPATIBLE_LABEL = "OpenAI compatible";

/** The OAuth account kinds as options; the record type is exhaustive as above. */
export const OAUTH_PROVIDER_TYPE_OPTIONS: ReadonlyArray<VocabularyOption<OAuthProviderType>> =
    Object.entries(OAUTH_PROVIDER_TYPE_LABELS).map(([value, label]) => ({
        value: value as OAuthProviderType,
        label,
    }));

/** The label a provider row's own `type` is presented under. */
export function providerTypeLabel(type: ProviderResponse["type"]): string {
    return type === "openai_compatible"
        ? OPENAI_COMPATIBLE_LABEL
        : OAUTH_PROVIDER_TYPE_LABELS[type];
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

export function findUpstreamFormat(value: unknown): UpstreamFormat | null {
    return findOption(UPSTREAM_FORMAT_OPTIONS, value);
}

export function findOAuthProviderType(value: unknown): OAuthProviderType | null {
    return findOption(OAUTH_PROVIDER_TYPE_OPTIONS, value);
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

/** Narrows a wire value that must be one of the known upstream formats. */
export function parseUpstreamFormat(value: unknown, context: string): UpstreamFormat {
    const format = findUpstreamFormat(value);
    if (format === null) {
        throw new ContractViolationError(
            `${context} is not a known upstream format: ${String(value)}`,
        );
    }
    return format;
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
