import { ContractViolationError } from "/src/management-api/errors";

import type {
    ApiLogEndpointType,
    ApiLogOutcome,
    DiscoveryState,
    ProviderResponse,
    UpstreamFormat,
    UsageGroupBy,
} from "/src/management-api/generated/v0";

/**
 * The credential kinds a provider row or a definition carries. The generated
 * bindings render the kinds as their literal union discriminants, so the
 * vocabulary states them here — the `ProviderResponse` union's own `type`
 * members keep the record below exhaustive.
 */
export type CredentialKind = "api_key" | "oauth";

type VocabularyOption<TValue extends string> = {
    readonly value: TValue;
    readonly label: string;
};

/**
 * The credential kinds with the label each is presented under. The record
 * type is exhaustive, so a kind added to the generated union fails to compile
 * until it is listed here.
 */
const CREDENTIAL_KIND_LABELS: Record<CredentialKind, string> = {
    api_key: "Static API key",
    oauth: "OAuth account",
};

/** The credential kinds as options; the record type is exhaustive as above. */
const CREDENTIAL_KIND_OPTIONS: ReadonlyArray<VocabularyOption<CredentialKind>> = [
    {
        value: "api_key",
        label: CREDENTIAL_KIND_LABELS.api_key,
    },
    {
        value: "oauth",
        label: CREDENTIAL_KIND_LABELS.oauth,
    },
];

/** The label a provider row's credential kind is presented under. */
export function credentialKindLabel(kind: CredentialKind): string {
    return CREDENTIAL_KIND_LABELS[kind];
}

/**
 * The upstream wire formats with the label each is presented under. The
 * record type is exhaustive, so a format added to the generated union fails
 * to compile until it is listed here.
 */
const UPSTREAM_FORMAT_LABELS: Record<UpstreamFormat, string> = {
    openai_response: "OpenAI Responses",
    openai_chat_completion: "OpenAI Chat Completions",
    gemini_generate_content: "Gemini GenerateContent",
    gemini_interaction: "Gemini Interactions",
    anthropic_messages: "Anthropic Messages",
};

/** The upstream wire formats as options; the record type is exhaustive as above. */
const UPSTREAM_FORMAT_OPTIONS: ReadonlyArray<VocabularyOption<UpstreamFormat>> = Object.entries(
    UPSTREAM_FORMAT_LABELS,
).map(([value, label]) => ({
    value: value as UpstreamFormat,
    label,
}));

/** The label one wire format is presented under. */
export function upstreamFormatLabel(format: UpstreamFormat): string {
    return UPSTREAM_FORMAT_LABELS[format];
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

/** Looks a credential kind up, returning `null` when the value is not one. */
export function findCredentialKind(value: unknown): CredentialKind | null {
    return findOption(CREDENTIAL_KIND_OPTIONS, value);
}

/** Looks a wire format up, returning `null` when the value is not one. */
export function findUpstreamFormat(value: unknown): UpstreamFormat | null {
    return findOption(UPSTREAM_FORMAT_OPTIONS, value);
}

/**
 * Whether a stored row is an OAuth account: exactly the variant carrying
 * `expires_at`. Narrows the row, so the expiry is readable past the guard.
 */
export function isOAuthProviderRow(provider: ProviderResponse): provider is Extract<
    ProviderResponse,
    {
        type: "oauth";
    }
> {
    return provider.type === "oauth";
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

/** The recorded request outcomes as filter options; the record type is exhaustive. */
export const API_LOG_OUTCOME_OPTIONS: ReadonlyArray<VocabularyOption<ApiLogOutcome>> =
    Object.entries(API_LOG_OUTCOME_LABELS).map(([value, label]) => ({
        value: value as ApiLogOutcome,
        label,
    }));

/** The label one accounting row's own `outcome` is presented under. */
export function apiLogOutcomeLabel(outcome: ApiLogOutcome): string {
    return API_LOG_OUTCOME_LABELS[outcome];
}

/** Looks a wire value up as a recorded outcome, answering `null` when it is not one. */
export function findApiLogOutcome(value: unknown): ApiLogOutcome | null {
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
    gemini_generate_content: "Gemini generateContent",
    gemini_interactions: "Gemini Interactions",
    anthropic_messages: "Anthropic Messages",
};

/** The outward endpoints as filter options; the record type is exhaustive. */
export const API_LOG_ENDPOINT_OPTIONS: ReadonlyArray<VocabularyOption<ApiLogEndpointType>> =
    Object.entries(API_LOG_ENDPOINT_LABELS).map(([value, label]) => ({
        value: value as ApiLogEndpointType,
        label,
    }));

/** The label one accounting row's own `endpoint_type` is presented under. */
export function apiLogEndpointLabel(endpoint: ApiLogEndpointType): string {
    return API_LOG_ENDPOINT_LABELS[endpoint];
}

/** Looks a wire value up as an outward endpoint, answering `null` when it is not one. */
export function findApiLogEndpointType(value: unknown): ApiLogEndpointType | null {
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
    endpoint_type: "Endpoint",
    outcome: "Outcome",
    is_stream: "Stream mode",
    http_status: "HTTP status",
};

/** The aggregation dimensions as options; the record type is exhaustive. */
export const USAGE_GROUP_BY_OPTIONS: ReadonlyArray<VocabularyOption<UsageGroupBy>> = Object.entries(
    USAGE_GROUP_BY_LABELS,
).map(([value, label]) => ({
    value: value as UsageGroupBy,
    label,
}));
