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
 * members keep the type below exhaustive.
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

/**
 * One closed wire vocabulary raised from its labels record: the select
 * options, the display labels, and the wire-value narrows behind one named
 * constant. The record type is exhaustive, so a member added to the generated
 * union fails to compile until it is listed there — the explicit `TValue` at
 * each call site ties the record to the contract's own union instead of
 * letting inference shrink it to the literal's keys.
 *
 * The `noun` is the value's noun phrase as `parse`'s violation names it,
 * article included — e.g. `"a discovery state"` — so every parser's message
 * stays byte-identical.
 *
 * Every member closes over the built options and the labels record, so the
 * returned methods are plain functions: passing one unbound — a validator
 * table's `narrow`, say — never depends on the receiver.
 */
function defineVocabulary<TValue extends string>(
    labels: Record<TValue, string>,
    noun: string,
): {
    readonly options: ReadonlyArray<VocabularyOption<TValue>>;
    label(value: TValue): string;
    find(value: unknown): TValue | null;
    parse(value: unknown, context: string): TValue;
} {
    const options = optionsOf(labels);

    return {
        options,
        label: (value) => labels[value],
        find: (value) => findOption(options, value),
        parse: (value, context) => parseOption(options, value, context, noun),
    };
}

/**
 * The credential kinds as one vocabulary. The labels record's type is
 * exhaustive, so a kind added to the generated union fails to compile until
 * it is listed here.
 */
export const CREDENTIAL_KIND = defineVocabulary<CredentialKind>(
    {
        api_key: "Static API key",
        oauth: "OAuth account",
    },
    "a credential kind",
);

/**
 * The upstream wire formats as one vocabulary. The labels record's type is
 * exhaustive, so a format added to the generated union fails to compile until
 * it is listed here.
 */
export const UPSTREAM_FORMAT = defineVocabulary<UpstreamFormat>(
    {
        openai_response: "OpenAI Responses",
        openai_chat_completion: "OpenAI Chat Completions",
        gemini_generate_content: "Gemini GenerateContent",
        gemini_interaction: "Gemini Interactions",
        anthropic_messages: "Anthropic Messages",
    },
    "an upstream format",
);

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
 * The discovery lifecycle states as one vocabulary. The labels record's type
 * is exhaustive, so a state added to the generated union fails to compile
 * until it is listed here.
 */
export const DISCOVERY_STATE = defineVocabulary<DiscoveryState>(
    {
        discovered: "Discovered",
        // The wire value `vanished` never reaches the operator verbatim: the
        // label states what it means for them — the upstream no longer lists
        // this model, while everything curated here is kept.
        vanished: "No longer reported",
        manual: "Manual",
    },
    "a discovery state",
);

/** The recorded request outcomes as one vocabulary; the labels record's type is exhaustive. */
export const API_LOG_OUTCOME = defineVocabulary<ApiLogOutcome>(
    {
        completed: "Completed",
        client_aborted: "Client aborted",
        upstream_error: "Upstream error",
        internal: "Internal",
    },
    "a request outcome",
);

/** The outward endpoints as one vocabulary; the labels record's type is exhaustive. */
export const API_LOG_ENDPOINT_TYPE = defineVocabulary<ApiLogEndpointType>(
    {
        openai_chat_completions: "Chat Completions",
        openai_responses: "Responses",
        gemini_generate_content: "Gemini GenerateContent",
        gemini_interactions: "Gemini Interactions",
        anthropic_messages: "Anthropic Messages",
    },
    "an endpoint type",
);

/** The aggregation dimensions as one vocabulary; the labels record's type is exhaustive. */
export const USAGE_GROUP_BY = defineVocabulary<UsageGroupBy>(
    {
        day: "Day",
        model: "Model",
        provider: "Provider",
        api_key: "API key",
        endpoint_type: "Endpoint",
        outcome: "Outcome",
        is_stream: "Stream mode",
        http_status: "HTTP status",
    },
    "an aggregation dimension",
);
