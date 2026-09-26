import { ContractViolationError } from "/src/management-api/errors";

import {
    type ApiKeyEnvelope,
    type ApiKeyResponse,
    type ApiLogResponse,
    type CompleteProviderOauthResponse,
    type DiscoverModelsResponse,
    type EmptyResponse,
    ErrorCode,
    type ErrorResponse,
    type ListApiKeysResponse,
    type ListModelsResponse,
    type ListProvidersResponse,
    type ListUsageResponse,
    type ModelEnvelope,
    type ModelResponse,
    type ProviderEnvelope,
    type ProviderResponse,
    type QueryUsageResponse,
    type StartProviderOauthResponse,
    type UsageBucket,
    type UsageBucketKey,
} from "/src/management-api/generated/v0";

import {
    findCredentialKind,
    findUpstreamFormat,
    parseApiLogEndpointType,
    parseApiLogOutcome,
    parseDiscoveryState,
} from "/src/management-api/vocabulary";

/**
 * Response validation (README §4.6): every management response enters the app
 * as `unknown` and is narrowed here — object shape, required keys, value types.
 * A body that does not match the generated bindings is a contract violation,
 * never partially rendered data.
 *
 * Unknown extra keys are tolerated: the bindings type only what the version
 * promises, and a newer gateway adding fields stays readable. The authored
 * definition payloads are the exception — the gateway parses descriptors with
 * `deny_unknown_fields`, so their walk names an undeclared key instead of
 * tolerating it; that walk and the stored-definition rows it rebuilds live in
 * `definition-payload.ts`, the descriptor contract's own module.
 *
 * This module is also the primitives home: the scalar `expect*` narrowings and
 * the upstream-format parse below are what `definition-payload.ts`'s
 * descriptor walk builds on.
 */
export function expectRecord(value: unknown, context: string): object {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
        throw new ContractViolationError(`${context} must be an object`);
    }

    return value;
}

export function expectArray(value: unknown, context: string): unknown[] {
    if (!Array.isArray(value)) {
        throw new ContractViolationError(`${context} must be an array`);
    }

    return value;
}

export function expectString(value: unknown, context: string): string {
    if (typeof value !== "string") {
        throw new ContractViolationError(`${context} must be a string`);
    }

    return value;
}

export function expectBoolean(value: unknown, context: string): boolean {
    if (typeof value !== "boolean") {
        throw new ContractViolationError(`${context} must be a boolean`);
    }

    return value;
}

export function expectInteger(value: unknown, context: string): number {
    if (typeof value !== "number" || !Number.isInteger(value)) {
        throw new ContractViolationError(`${context} must be an integer`);
    }

    return value;
}

function expectNullableString(value: unknown, context: string): string | null {
    if (value === null) {
        return null;
    }

    if (typeof value !== "string") {
        throw new ContractViolationError(`${context} must be a string or null`);
    }

    return value;
}

function expectNullableInteger(value: unknown, context: string): number | null {
    if (value === null) {
        return null;
    }

    if (typeof value !== "number" || !Number.isInteger(value)) {
        throw new ContractViolationError(`${context} must be an integer or null`);
    }

    return value;
}

/**
 * One composite-key dimension: its wire field name and the narrowing that
 * types a present value. The narrowed type correlates with the name through
 * `UsageBucketKey`, so a dimension added to the contract extends the table
 * below, not the walk, and a mispaired narrowing cannot compile.
 */
type BucketKeyField = {
    readonly [TFieldName in keyof UsageBucketKey]-?: {
        readonly name: TFieldName;
        readonly narrow: (value: unknown, context: string) => UsageBucketKey[TFieldName];
    };
}[keyof UsageBucketKey];

/** Every dimension the composite bucket key can carry, in bindings order. */
const BUCKET_KEY_FIELDS: ReadonlyArray<BucketKeyField> = [
    {
        name: "day",
        narrow: expectString,
    },
    {
        name: "model_id",
        narrow: expectInteger,
    },
    {
        name: "provider_id",
        narrow: expectInteger,
    },
    {
        name: "api_key_id",
        narrow: expectInteger,
    },
    {
        name: "endpoint_type",
        narrow: parseApiLogEndpointType,
    },
    {
        name: "outcome",
        narrow: parseApiLogOutcome,
    },
    {
        name: "is_stream",
        narrow: expectBoolean,
    },
    {
        name: "http_status",
        narrow: expectNullableInteger,
    },
];

/**
 * Types one present dimension into the key through its correlated narrowing;
 * an absent one passes through, exactly as the wire omits it.
 */
function writeBucketKeyField<TFieldName extends keyof UsageBucketKey>(
    key: UsageBucketKey,
    field: {
        readonly name: TFieldName;
        readonly narrow: (value: unknown, context: string) => UsageBucketKey[TFieldName];
    },
    record: object,
    context: string,
): void {
    const value = Reflect.get(record, field.name);

    if (value !== undefined) {
        key[field.name] = field.narrow(value, `${context}.${field.name}`);
    }
}

function expectBucketKey(value: unknown, context: string): UsageBucketKey {
    const record = expectRecord(value, context);
    const key: UsageBucketKey = {};

    for (const field of BUCKET_KEY_FIELDS) {
        writeBucketKeyField(key, field, record, context);
    }

    return key;
}

/**
 * The definition pair, the derived format, and the nullable base URL: the
 * fields both credential kinds share, flattened into each union member.
 */
function parseSharedProviderFields(
    record: object,
    context: string,
): {
    readonly id: number;
    readonly name: string;
    readonly model_prefix: string;
    readonly is_enabled: boolean;
    readonly provider_definition_name: string;
    readonly provider_definition_version: number;
    readonly format: ProviderResponse["format"];
    readonly base_url: string | null;
} {
    return {
        id: expectInteger(Reflect.get(record, "id"), `${context}.id`),
        name: expectString(Reflect.get(record, "name"), `${context}.name`),
        model_prefix: expectString(Reflect.get(record, "model_prefix"), `${context}.model_prefix`),
        is_enabled: expectBoolean(Reflect.get(record, "is_enabled"), `${context}.is_enabled`),
        provider_definition_name: expectString(
            Reflect.get(record, "provider_definition_name"),
            `${context}.provider_definition_name`,
        ),
        provider_definition_version: expectInteger(
            Reflect.get(record, "provider_definition_version"),
            `${context}.provider_definition_version`,
        ),
        format: parseUpstreamFormat(Reflect.get(record, "format"), `${context}.format`),
        base_url: expectNullableString(Reflect.get(record, "base_url"), `${context}.base_url`),
    };
}

/**
 * One provider row; the union is discriminated by the row's credential kind.
 * Both members share the definition pair and the derived format; the OAuth
 * member alone carries the nullable `expires_at`. The kind membership reads
 * from the closed vocabulary, so a kind added to the contract cannot compile
 * past it and then fail here at runtime.
 */
function parseProviderResponse(value: unknown, context: string): ProviderResponse {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);
    const shared = parseSharedProviderFields(record, context);

    const kind = findCredentialKind(type);

    if (kind === "oauth") {
        return {
            type: kind,
            ...shared,
            expires_at: expectNullableInteger(
                Reflect.get(record, "expires_at"),
                `${context}.expires_at`,
            ),
        };
    }

    if (kind === "api_key") {
        return {
            type: kind,
            ...shared,
        };
    }

    throw new ContractViolationError(`${context}.type is not a known credential kind: ${type}`);
}

/** Narrows a wire value that must be one of the five upstream wire formats. */
export function parseUpstreamFormat(value: unknown, context: string): ProviderResponse["format"] {
    const format = findUpstreamFormat(value);

    if (format === null) {
        throw new ContractViolationError(`${context} is not an upstream format: ${String(value)}`);
    }

    return format;
}

/**
 * One model row: its never-recycled row identifier, its routing string, and
 * its own inputs — the nullable mirror column, the serving switch, the
 * discovery state. The effective verdict is never on the wire (rule 7).
 */
function parseModelResponse(value: unknown, context: string): ModelResponse {
    const record = expectRecord(value, context);

    return {
        id: expectInteger(Reflect.get(record, "id"), `${context}.id`),
        provider_id: expectInteger(Reflect.get(record, "provider_id"), `${context}.provider_id`),
        provider: expectString(Reflect.get(record, "provider"), `${context}.provider`),
        upstream_id: expectString(Reflect.get(record, "upstream_id"), `${context}.upstream_id`),
        public_id: expectString(Reflect.get(record, "public_id"), `${context}.public_id`),
        id_override: expectNullableString(
            Reflect.get(record, "id_override"),
            `${context}.id_override`,
        ),
        is_enabled: expectBoolean(Reflect.get(record, "is_enabled"), `${context}.is_enabled`),
        discovery_state: parseDiscoveryState(
            Reflect.get(record, "discovery_state"),
            `${context}.discovery_state`,
        ),
    };
}

/**
 * One accounting row, verbatim: the row's own fourteen published columns,
 * stamps and numeric facts only — no row id, no name rides along (rule 7).
 */
function parseApiLogResponse(value: unknown, context: string): ApiLogResponse {
    const record = expectRecord(value, context);

    return {
        started_at: expectInteger(Reflect.get(record, "started_at"), `${context}.started_at`),
        api_key_id: expectInteger(Reflect.get(record, "api_key_id"), `${context}.api_key_id`),
        provider_id: expectInteger(Reflect.get(record, "provider_id"), `${context}.provider_id`),
        model_id: expectInteger(Reflect.get(record, "model_id"), `${context}.model_id`),
        endpoint_type: parseApiLogEndpointType(
            Reflect.get(record, "endpoint_type"),
            `${context}.endpoint_type`,
        ),
        is_stream: expectBoolean(Reflect.get(record, "is_stream"), `${context}.is_stream`),
        http_status: expectNullableInteger(
            Reflect.get(record, "http_status"),
            `${context}.http_status`,
        ),
        outcome: parseApiLogOutcome(Reflect.get(record, "outcome"), `${context}.outcome`),
        duration: expectInteger(Reflect.get(record, "duration"), `${context}.duration`),
        input_tokens: expectNullableInteger(
            Reflect.get(record, "input_tokens"),
            `${context}.input_tokens`,
        ),
        output_tokens: expectNullableInteger(
            Reflect.get(record, "output_tokens"),
            `${context}.output_tokens`,
        ),
        cached_tokens: expectNullableInteger(
            Reflect.get(record, "cached_tokens"),
            `${context}.cached_tokens`,
        ),
        cache_write_tokens: expectNullableInteger(
            Reflect.get(record, "cache_write_tokens"),
            `${context}.cache_write_tokens`,
        ),
        reasoning_tokens: expectNullableInteger(
            Reflect.get(record, "reasoning_tokens"),
            `${context}.reasoning_tokens`,
        ),
    };
}

/** One aggregation bucket: the grouping key and the log's own numeric sums. */
function parseUsageBucket(value: unknown, context: string): UsageBucket {
    const record = expectRecord(value, context);

    return {
        key: expectBucketKey(Reflect.get(record, "key"), `${context}.key`),
        requests: expectInteger(Reflect.get(record, "requests"), `${context}.requests`),
        input_tokens: expectInteger(Reflect.get(record, "input_tokens"), `${context}.input_tokens`),
        output_tokens: expectInteger(
            Reflect.get(record, "output_tokens"),
            `${context}.output_tokens`,
        ),
        cached_tokens: expectInteger(
            Reflect.get(record, "cached_tokens"),
            `${context}.cached_tokens`,
        ),
        cache_write_tokens: expectInteger(
            Reflect.get(record, "cache_write_tokens"),
            `${context}.cache_write_tokens`,
        ),
        reasoning_tokens: expectInteger(
            Reflect.get(record, "reasoning_tokens"),
            `${context}.reasoning_tokens`,
        ),
    };
}

export function parseListProvidersResponse(value: unknown): ListProvidersResponse {
    const record = expectRecord(value, "response");
    const providers = expectArray(Reflect.get(record, "providers"), "response.providers");

    return {
        providers: providers.map((entry, index) =>
            parseProviderResponse(entry, `response.providers[${index}]`),
        ),
    };
}

export function parseProviderEnvelope(value: unknown): ProviderEnvelope {
    const record = expectRecord(value, "response");

    return {
        provider: parseProviderResponse(Reflect.get(record, "provider"), "response.provider"),
    };
}

export function parseListModelsResponse(value: unknown): ListModelsResponse {
    const record = expectRecord(value, "response");
    const models = expectArray(Reflect.get(record, "models"), "response.models");

    return {
        models: models.map((entry, index) =>
            parseModelResponse(entry, `response.models[${index}]`),
        ),
    };
}

export function parseDiscoverModelsResponse(value: unknown): DiscoverModelsResponse {
    return parseListModelsResponse(value);
}

export function parseModelEnvelope(value: unknown): ModelEnvelope {
    const record = expectRecord(value, "response");

    return {
        model: parseModelResponse(Reflect.get(record, "model"), "response.model"),
    };
}

export function parseListUsageResponse(value: unknown): ListUsageResponse {
    const record = expectRecord(value, "response");
    const apiLogs = expectArray(Reflect.get(record, "api_logs"), "response.api_logs");

    return {
        api_logs: apiLogs.map((entry, index) =>
            parseApiLogResponse(entry, `response.api_logs[${index}]`),
        ),
    };
}

export function parseQueryUsageResponse(value: unknown): QueryUsageResponse {
    const record = expectRecord(value, "response");
    const buckets = expectArray(Reflect.get(record, "buckets"), "response.buckets");

    return {
        buckets: buckets.map((entry, index) =>
            parseUsageBucket(entry, `response.buckets[${index}]`),
        ),
    };
}

/** One gateway API key row: the complete readable state, plaintext secret included. */
function parseApiKeyResponse(value: unknown, context: string): ApiKeyResponse {
    const record = expectRecord(value, context);

    return {
        id: expectInteger(Reflect.get(record, "id"), `${context}.id`),
        name: expectString(Reflect.get(record, "name"), `${context}.name`),
        key: expectString(Reflect.get(record, "key"), `${context}.key`),
        is_enabled: expectBoolean(Reflect.get(record, "is_enabled"), `${context}.is_enabled`),
        created_at: expectInteger(Reflect.get(record, "created_at"), `${context}.created_at`),
    };
}

export function parseListApiKeysResponse(value: unknown): ListApiKeysResponse {
    const record = expectRecord(value, "response");
    const apiKeys = expectArray(Reflect.get(record, "api_keys"), "response.api_keys");

    return {
        api_keys: apiKeys.map((entry, index) =>
            parseApiKeyResponse(entry, `response.api_keys[${index}]`),
        ),
    };
}

export function parseApiKeyEnvelope(value: unknown): ApiKeyEnvelope {
    const record = expectRecord(value, "response");

    return {
        api_key: parseApiKeyResponse(Reflect.get(record, "api_key"), "response.api_key"),
    };
}

/**
 * One begun flow: a union discriminated by the grant, because an
 * authorization-code flow answers with the URL to open and a device flow
 * answers with the verification facts to approve through.
 */
export function parseStartProviderOauthResponse(value: unknown): StartProviderOauthResponse {
    const record = expectRecord(value, "response");
    const grant = expectString(Reflect.get(record, "grant"), "response.grant");
    const state = expectString(Reflect.get(record, "state"), "response.state");
    const expiresAt = expectInteger(Reflect.get(record, "expires_at"), "response.expires_at");

    if (grant === "authorization_code") {
        return {
            grant,
            state,
            expires_at: expiresAt,
            authorize_url: expectString(
                Reflect.get(record, "authorize_url"),
                "response.authorize_url",
            ),
        };
    }

    if (grant === "device") {
        const verificationUri = Reflect.get(record, "verification_uri");
        const verificationUriComplete = Reflect.get(record, "verification_uri_complete");

        return {
            grant,
            state,
            expires_at: expiresAt,
            user_code: expectString(Reflect.get(record, "user_code"), "response.user_code"),
            ...(verificationUri === undefined
                ? {}
                : {
                      verification_uri: expectString(verificationUri, "response.verification_uri"),
                  }),
            ...(verificationUriComplete === undefined
                ? {}
                : {
                      verification_uri_complete: expectString(
                          verificationUriComplete,
                          "response.verification_uri_complete",
                      ),
                  }),
        };
    }

    throw new ContractViolationError(`response.grant is not a known grant: ${grant}`);
}

/** `providers:update` and `providers:delete` answer an empty JSON object. */
export function parseEmptyResponse(value: unknown): EmptyResponse {
    expectRecord(value, "response");
    return {};
}

/**
 * The version's closed errno taxonomy, read from the generated enum's own
 * reverse mapping so the set cannot drift from the bindings.
 */
const ERROR_CODES: ReadonlySet<number> = new Set(
    Object.keys(ErrorCode)
        .map((name) => Reflect.get(ErrorCode, name))
        .filter((value): value is number => typeof value === "number"),
);

/**
 * Reads an errno-shaped error body. A body that is not one — a foreign
 * service's HTML error page, say — reads as `null`, and the caller reports the
 * HTTP status alone (README §4.2).
 */
export function parseErrorResponse(value: unknown): ErrorResponse | null {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
        return null;
    }

    const errno: unknown = Reflect.get(value, "errno");
    const message: unknown = Reflect.get(value, "message");

    if (typeof errno !== "number" || typeof message !== "string" || !ERROR_CODES.has(errno)) {
        return null;
    }

    // `ErrorCode`'s members are their own numbers, and the membership test above
    // is against the set built from those members.
    return {
        errno: errno as ErrorCode,
        message,
    };
}

/**
 * One finished or polled flow: succeeded carries the provider row, pending
 * carries nothing, failed carries the gateway-owned failure pair.
 */
export function parseCompleteProviderOauthResponse(value: unknown): CompleteProviderOauthResponse {
    const record = expectRecord(value, "response");
    const status = expectString(Reflect.get(record, "status"), "response.status");

    if (status === "succeeded") {
        return {
            status,
            provider: parseProviderResponse(Reflect.get(record, "provider"), "response.provider"),
        };
    }

    if (status === "pending") {
        return {
            status,
        };
    }

    if (status === "failed") {
        return {
            status,
            failure_code: expectString(
                Reflect.get(record, "failure_code"),
                "response.failure_code",
            ),
            failure_message: expectString(
                Reflect.get(record, "failure_message"),
                "response.failure_message",
            ),
        };
    }

    throw new ContractViolationError(`response.status is not a flow outcome: ${status}`);
}
