import { ContractViolationError } from "/src/management-api/errors";

import type {
    ApiLogEndpointType,
    ApiLogOutcome,
    DiscoveryState,
    OAuthProviderType,
    ProviderResponse,
    ProviderType,
    UsageGroupBy,
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

const OAUTH_PROVIDER_TYPE_LABELS: Record<OAuthProviderType, string> = {
    codex_oauth: "Codex OAuth",
    antigravity_oauth: "Antigravity OAuth",
};

/** The OAuth provider kinds as options; the record type is exhaustive as above. */
const OAUTH_PROVIDER_TYPE_OPTIONS: ReadonlyArray<VocabularyOption<OAuthProviderType>> =
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

const DISCOVERY_STATE_LABELS: Record<DiscoveryState, string> = {
    discovered: "Discovered",
    // The wire value `vanished` never reaches the operator verbatim: the
    // label states what it means for them — the upstream no longer lists
    // this model, while everything curated here is kept.
    vanished: "No longer reported",
    manual: "Manual",
};

/**
 * The discovery lifecycle states as options; the record type is exhaustive,
 * so a state added to the generated union fails to compile until it is
 * listed here.
 */
const DISCOVERY_STATE_OPTIONS: ReadonlyArray<VocabularyOption<DiscoveryState>> = Object.entries(
    DISCOVERY_STATE_LABELS,
).map(([value, label]) => ({
    value: value as DiscoveryState,
    label,
}));

/** The label a model row's own `discovery_state` is presented under. */
export function discoveryStateLabel(state: DiscoveryState): string {
    return DISCOVERY_STATE_LABELS[state];
}

function findDiscoveryState(value: unknown): DiscoveryState | null {
    return findOption(DISCOVERY_STATE_OPTIONS, value);
}

/** Narrows a wire value that must be one of the discovery lifecycle states. */
export function parseDiscoveryState(value: unknown, context: string): DiscoveryState {
    const state = findDiscoveryState(value);

    if (state === null) {
        throw new ContractViolationError(`${context} is not a discovery state: ${String(value)}`);
    }

    return state;
}

const API_LOG_OUTCOME_LABELS: Record<ApiLogOutcome, string> = {
    completed: "Completed",
    client_aborted: "Client aborted",
    upstream_error: "Upstream error",
    internal: "Internal",
};

/** The recorded request outcomes as options; the record type is exhaustive. */
const API_LOG_OUTCOME_OPTIONS: ReadonlyArray<VocabularyOption<ApiLogOutcome>> = Object.entries(
    API_LOG_OUTCOME_LABELS,
).map(([value, label]) => ({
    value: value as ApiLogOutcome,
    label,
}));

/** The label one accounting row's own `outcome` is presented under. */
export function apiLogOutcomeLabel(outcome: ApiLogOutcome): string {
    return API_LOG_OUTCOME_LABELS[outcome];
}

function findApiLogOutcome(value: unknown): ApiLogOutcome | null {
    return findOption(API_LOG_OUTCOME_OPTIONS, value);
}

/** Narrows a wire value that must be one of the recorded request outcomes. */
export function parseApiLogOutcome(value: unknown, context: string): ApiLogOutcome {
    const outcome = findApiLogOutcome(value);

    if (outcome === null) {
        throw new ContractViolationError(`${context} is not a request outcome: ${String(value)}`);
    }

    return outcome;
}

const API_LOG_ENDPOINT_LABELS: Record<ApiLogEndpointType, string> = {
    openai_chat_completions: "Chat Completions",
    openai_responses: "Responses",
};

/** The outward endpoints as options; the record type is exhaustive. */
const API_LOG_ENDPOINT_OPTIONS: ReadonlyArray<VocabularyOption<ApiLogEndpointType>> =
    Object.entries(API_LOG_ENDPOINT_LABELS).map(([value, label]) => ({
        value: value as ApiLogEndpointType,
        label,
    }));

/** The label one accounting row's own `endpoint_type` is presented under. */
export function apiLogEndpointLabel(endpoint: ApiLogEndpointType): string {
    return API_LOG_ENDPOINT_LABELS[endpoint];
}

function findApiLogEndpointType(value: unknown): ApiLogEndpointType | null {
    return findOption(API_LOG_ENDPOINT_OPTIONS, value);
}

/** Narrows a wire value that must be one of the outward endpoints. */
export function parseApiLogEndpointType(value: unknown, context: string): ApiLogEndpointType {
    const endpoint = findApiLogEndpointType(value);

    if (endpoint === null) {
        throw new ContractViolationError(`${context} is not an endpoint type: ${String(value)}`);
    }

    return endpoint;
}

const USAGE_GROUP_BY_LABELS: Record<UsageGroupBy, string> = {
    day: "Day",
    model: "Model",
    provider: "Provider",
    api_key: "API key",
};

/** The aggregation dimensions as options; the record type is exhaustive. */
export const USAGE_GROUP_BY_OPTIONS: ReadonlyArray<VocabularyOption<UsageGroupBy>> = Object.entries(
    USAGE_GROUP_BY_LABELS,
).map(([value, label]) => ({
    value: value as UsageGroupBy,
    label,
}));

/** The label a `usage:query` bucketing dimension is presented under. */
export function usageGroupByLabel(groupBy: UsageGroupBy): string {
    return USAGE_GROUP_BY_LABELS[groupBy];
}

function findUsageGroupBy(value: unknown): UsageGroupBy | null {
    return findOption(USAGE_GROUP_BY_OPTIONS, value);
}

/** Narrows a wire value that must be one of the aggregation dimensions. */
export function parseUsageGroupBy(value: unknown, context: string): UsageGroupBy {
    const groupBy = findUsageGroupBy(value);

    if (groupBy === null) {
        throw new ContractViolationError(`${context} is not a usage grouping: ${String(value)}`);
    }

    return groupBy;
}
