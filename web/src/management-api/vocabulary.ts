import { isCredentialExpired, nowMicros } from "/src/format";
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
 * A labels record as the options it presents. The record type is exhaustive,
 * so its keys are exactly the values of `TValue` — the single assertion below
 * restates what the signature already guarantees.
 */
function optionsOf<TValue extends string>(
    labels: Record<TValue, string>,
): ReadonlyArray<VocabularyOption<TValue>> {
    return (Object.keys(labels) as TValue[]).map((value) => ({
        value,
        label: labels[value],
    }));
}

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
const CREDENTIAL_KIND_OPTIONS: ReadonlyArray<VocabularyOption<CredentialKind>> =
    optionsOf(CREDENTIAL_KIND_LABELS);

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
const UPSTREAM_FORMAT_OPTIONS: ReadonlyArray<VocabularyOption<UpstreamFormat>> =
    optionsOf(UPSTREAM_FORMAT_LABELS);

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

/**
 * Narrows a wire value that must be one of a vocabulary's members, naming the
 * offending value in a `ContractViolationError` otherwise. The noun is the
 * value's noun phrase as the violation names it, article included — e.g.
 * `"a discovery state"` — so every parser's message stays byte-identical.
 */
function parseOption<TValue extends string>(
    options: ReadonlyArray<VocabularyOption<TValue>>,
    value: unknown,
    context: string,
    noun: string,
): TValue {
    const member = findOption(options, value);

    if (member === null) {
        throw new ContractViolationError(`${context} is not ${noun}: ${String(value)}`);
    }

    return member;
}

/** Looks a credential kind up, returning `null` when the value is not one. */
export function findCredentialKind(value: unknown): CredentialKind | null {
    return findOption(CREDENTIAL_KIND_OPTIONS, value);
}

/** Looks a wire format up, returning `null` when the value is not one. */
export function findUpstreamFormat(value: unknown): UpstreamFormat | null {
    return findOption(UPSTREAM_FORMAT_OPTIONS, value);
}

/** The OAuth member of the provider row union — the only kind carrying an expiry. */
export type OauthProviderRow = Extract<
    ProviderResponse,
    {
        type: "oauth";
    }
>;

/**
 * Whether a stored row is an OAuth account: exactly the variant carrying
 * `expires_at`. Narrows the row, so the expiry is readable past the guard.
 */
export function isOAuthProviderRow(provider: ProviderResponse): provider is OauthProviderRow {
    return provider.type === "oauth";
}

/**
 * A provider row's credential standing — the one verdict every credential
 * presentation branches on: `"valid"` while a stored OAuth expiry still
 * holds, `"expired"` once it has passed, `"never-fresh"` when an OAuth row
 * stores no expiry at all, and `null` for a static-key row, which never
 * expires and so has no credential standing to state.
 */
export type CredentialStanding = "valid" | "expired" | "never-fresh" | null;

/**
 * Derives a provider row's credential standing from its own columns — the
 * single home of the verdict the providers table's credential badge, the
 * provider page's reconnect notice, and the overview's lapsed-credential
 * attention each branch on, so the three presentations cannot drift. An
 * OAuth row with no stored expiry was never fresh, an expiry already past
 * at the reading moment is expired, one still ahead is valid, and a static
 * key — which never expires — states no standing at all.
 */
export function credentialStanding(provider: ProviderResponse): CredentialStanding {
    if (!isOAuthProviderRow(provider)) {
        return null;
    }

    if (provider.expires_at === null) {
        return "never-fresh";
    }

    return isCredentialExpired(provider.expires_at, nowMicros()) ? "expired" : "valid";
}

/**
 * Whether a credential standing states a credential the gateway cannot use as
 * stored — the "needs a human" verdict over the standing: `"expired"` once
 * the stored OAuth expiry has passed, or `"never-fresh"` when an OAuth row
 * stores no expiry at all. A `"valid"` login is not lapsed, and a static
 * key — which never expires and states no standing at all — never is. Takes
 * the standing rather than the row so every presentation derives the
 * standing once, from `credentialStanding`, and shares this one disjunction
 * over it.
 */
export function isLapsedCredentialStanding(
    standing: CredentialStanding,
): standing is "expired" | "never-fresh" {
    return standing === "expired" || standing === "never-fresh";
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
const DISCOVERY_STATE_OPTIONS: ReadonlyArray<VocabularyOption<DiscoveryState>> =
    optionsOf(DISCOVERY_STATE_LABELS);

/** The label a model row's own `discovery_state` is presented under. */
export function discoveryStateLabel(state: DiscoveryState): string {
    return DISCOVERY_STATE_LABELS[state];
}

/** Narrows a wire value that must be one of the discovery lifecycle states. */
export function parseDiscoveryState(value: unknown, context: string): DiscoveryState {
    return parseOption(DISCOVERY_STATE_OPTIONS, value, context, "a discovery state");
}

const API_LOG_OUTCOME_LABELS: Record<ApiLogOutcome, string> = {
    completed: "Completed",
    client_aborted: "Client aborted",
    upstream_error: "Upstream error",
    internal: "Internal",
};

/** The recorded request outcomes as filter options; the record type is exhaustive. */
export const API_LOG_OUTCOME_OPTIONS: ReadonlyArray<VocabularyOption<ApiLogOutcome>> =
    optionsOf(API_LOG_OUTCOME_LABELS);

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
    return parseOption(API_LOG_OUTCOME_OPTIONS, value, context, "a request outcome");
}

const API_LOG_ENDPOINT_LABELS: Record<ApiLogEndpointType, string> = {
    openai_chat_completions: "Chat Completions",
    openai_responses: "Responses",
    gemini_generate_content: "Gemini GenerateContent",
    gemini_interactions: "Gemini Interactions",
    anthropic_messages: "Anthropic Messages",
};

/** The outward endpoints as filter options; the record type is exhaustive. */
export const API_LOG_ENDPOINT_OPTIONS: ReadonlyArray<VocabularyOption<ApiLogEndpointType>> =
    optionsOf(API_LOG_ENDPOINT_LABELS);

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
    return parseOption(API_LOG_ENDPOINT_OPTIONS, value, context, "an endpoint type");
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
export const USAGE_GROUP_BY_OPTIONS: ReadonlyArray<VocabularyOption<UsageGroupBy>> =
    optionsOf(USAGE_GROUP_BY_LABELS);
