import { ContractViolationError } from "/src/management-api/errors";

import {
    type ApiKeyEnvelope,
    type ApiKeyResponse,
    type ApiLogResponse,
    type CompleteProviderOauthResponse,
    type DefinitionEnvelope,
    type DefinitionResponse,
    type DiscoverModelsResponse,
    type EmptyResponse,
    ErrorCode,
    type ErrorResponse,
    type ListApiKeysResponse,
    type ListModelsResponse,
    type ListProviderDefinitionsResponse,
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
 * promises, and a newer gateway adding fields stays readable.
 */
function expectRecord(value: unknown, context: string): object {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
        throw new ContractViolationError(`${context} must be an object`);
    }

    return value;
}

function expectArray(value: unknown, context: string): unknown[] {
    if (!Array.isArray(value)) {
        throw new ContractViolationError(`${context} must be an array`);
    }

    return value;
}

function expectString(value: unknown, context: string): string {
    if (typeof value !== "string") {
        throw new ContractViolationError(`${context} must be a string`);
    }

    return value;
}

function expectBoolean(value: unknown, context: string): boolean {
    if (typeof value !== "boolean") {
        throw new ContractViolationError(`${context} must be a boolean`);
    }

    return value;
}

function expectInteger(value: unknown, context: string): number {
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

function expectBucketKey(value: unknown, context: string): UsageBucketKey {
    const record = expectRecord(value, context);
    const key: UsageBucketKey = {};

    const day = Reflect.get(record, "day");
    if (day !== undefined) {
        key.day = expectString(day, `${context}.day`);
    }

    const modelId = Reflect.get(record, "model_id");
    if (modelId !== undefined) {
        key.model_id = expectInteger(modelId, `${context}.model_id`);
    }

    const providerId = Reflect.get(record, "provider_id");
    if (providerId !== undefined) {
        key.provider_id = expectInteger(providerId, `${context}.provider_id`);
    }

    const apiKeyId = Reflect.get(record, "api_key_id");
    if (apiKeyId !== undefined) {
        key.api_key_id = expectInteger(apiKeyId, `${context}.api_key_id`);
    }

    const endpointType = Reflect.get(record, "endpoint_type");
    if (endpointType !== undefined) {
        key.endpoint_type = parseApiLogEndpointType(endpointType, `${context}.endpoint_type`);
    }

    const outcome = Reflect.get(record, "outcome");
    if (outcome !== undefined) {
        key.outcome = parseApiLogOutcome(outcome, `${context}.outcome`);
    }

    const isStream = Reflect.get(record, "is_stream");
    if (isStream !== undefined) {
        key.is_stream = expectBoolean(isStream, `${context}.is_stream`);
    }

    const httpStatus = Reflect.get(record, "http_status");
    if (httpStatus !== undefined) {
        key.http_status = expectNullableInteger(httpStatus, `${context}.http_status`);
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
function parseUpstreamFormat(value: unknown, context: string): ProviderResponse["format"] {
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
 * One accounting row, verbatim: the table's own fifteen columns, ids and
 * numeric facts only — no name rides along (rule 7).
 */
function parseApiLogResponse(value: unknown, context: string): ApiLogResponse {
    const record = expectRecord(value, context);

    return {
        id: expectInteger(Reflect.get(record, "id"), `${context}.id`),
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
 * The provider-definition descriptors (README §5): a definition payload is
 * authored JSON, so the parser walks its structure once — every closed
 * vocabulary is checked against the generated types, every expression and
 * template node against its own node kind. A payload the gateway would reject
 * at write time fails here the same way, as a contract violation.
 */
function expectExpressionParts(value: unknown, context: string): unknown[] {
    return expectArray(value, context).map((part, index) =>
        expectExpressionPart(part, `${context}[${index}]`),
    );
}

function expectExpressionPart(part: unknown, context: string): unknown {
    const record = expectRecord(part, context);
    const kind = expectString(Reflect.get(record, "type"), `${context}.type`);

    const slots = new Set([
        "literal",
        "secret",
        "slot",
        "client_id",
        "client_secret",
        "authorization_code",
        "code_verifier",
        "redirect_uri",
        "oauth_state",
        "device_code",
        "refresh_token",
        "request_uuid",
        "model",
        "base",
        "coalesce",
    ]);

    if (!slots.has(kind)) {
        throw new ContractViolationError(`${context}.type is not an expression kind: ${kind}`);
    }

    if (kind === "literal" || kind === "slot") {
        expectString(
            Reflect.get(record, kind === "literal" ? "value" : "name"),
            kind === "literal" ? `${context}.value` : `${context}.name`,
        );
    }

    if (kind === "coalesce") {
        expectExpressionParts(Reflect.get(record, "parts"), `${context}.parts`);
    }

    return part;
}

/** Validates one string expression: an ordered array of expression parts. */
function expectStringExpr(value: unknown, context: string): unknown[] {
    return expectExpressionParts(value, context);
}

/** Validates one param row: a name and its expression. */
function expectParamRows(value: unknown, context: string): unknown[] {
    return expectArray(value, context).map((row, index) => {
        const record = expectRecord(row, `${context}[${index}]`);
        expectString(Reflect.get(record, "name"), `${context}[${index}].name`);
        expectStringExpr(Reflect.get(record, "value"), `${context}[${index}].value`);

        return row;
    });
}

/** Validates one template tree node, body placeholders included. */
function expectTemplateNode(value: unknown, context: string): unknown {
    const record = expectRecord(value, context);
    const kind = expectString(Reflect.get(record, "type"), `${context}.type`);

    switch (kind) {
        case "json":
            Reflect.get(record, "value");
            return value;

        case "string":
            expectStringExpr(Reflect.get(record, "expr"), `${context}.expr`);
            return value;

        case "body":
            return value;

        case "object": {
            const fields = expectRecord(
                Reflect.get(record, "fields"),
                `${context}.fields`,
            );

            for (const name of Object.keys(fields)) {
                expectTemplateNode(
                    Reflect.get(fields, name),
                    `${context}.fields.${name}`,
                );
            }

            return value;
        }

        case "array":
            expectArray(Reflect.get(record, "items"), `${context}.items`).map((item, index) =>
                expectTemplateNode(item, `${context}.items[${index}]`),
            );
            return value;

        default:
            throw new ContractViolationError(`${context}.type is not a template node: ${kind}`);
    }
}

/** Validates one control request: method, URL, headers, query, and body. */
function expectControlRequest(value: unknown, context: string): void {
    const record = expectRecord(value, context);
    const method = expectString(Reflect.get(record, "method"), `${context}.method`);

    if (method !== "GET" && method !== "POST") {
        throw new ContractViolationError(`${context}.method is not a control method: ${method}`);
    }

    expectStringExpr(Reflect.get(record, "url"), `${context}.url`);

    for (const key of ["headers", "query"] as const) {
        const rows = Reflect.get(record, key);
        if (rows !== undefined) {
            expectParamRows(rows, `${context}.${key}`);
        }
    }

    const body = Reflect.get(record, "body");

    if (body === undefined || body === null) {
        return;
    }

    const bodyRecord = expectRecord(body, `${context}.body`);
    const bodyKind = expectString(Reflect.get(bodyRecord, "kind"), `${context}.body.kind`);

    if (bodyKind === "form") {
        expectParamRows(Reflect.get(bodyRecord, "fields"), `${context}.body.fields`);
    } else if (bodyKind === "json") {
        expectTemplateNode(Reflect.get(bodyRecord, "node"), `${context}.body.node`);
    } else {
        throw new ContractViolationError(`${context}.body.kind is not a control body: ${bodyKind}`);
    }
}

/** Validates one response path: a chain of non-empty member names. */
function expectResponsePath(value: unknown, context: string): void {
    for (const [index, member] of expectArray(value, context).entries()) {
        expectString(member, `${context}[${index}]`);
    }
}

/** Validates one provisioning sequence with its polls. */
function expectProvisioning(value: unknown, context: string): void {
    const steps = expectArray(value, context);

    for (const [index, step] of steps.entries()) {
        const record = expectRecord(step, `${context}[${index}]`);
        expectControlRequest(Reflect.get(record, "request"), `${context}[${index}].request`);

        const encoding = expectString(
            Reflect.get(record, "response_encoding"),
            `${context}[${index}].response_encoding`,
        );

        if (encoding !== "json" && encoding !== "form") {
            throw new ContractViolationError(
                `${context}[${index}].response_encoding is not a response encoding`,
            );
        }

        const extractions = Reflect.get(record, "extract");

        if (extractions !== undefined) {
            for (const [extractionIndex, extraction] of expectArray(
                extractions,
                `${context}[${index}].extract`,
            ).entries()) {
                const extractionRecord = expectRecord(
                    extraction,
                    `${context}[${index}].extract[${extractionIndex}]`,
                );
                expectResponsePath(
                    Reflect.get(extractionRecord, "path"),
                    `${context}[${index}].extract[${extractionIndex}].path`,
                );
            }
        }

        const poll = Reflect.get(record, "poll");

        if (poll !== undefined) {
            const pollRecord = expectRecord(poll, `${context}[${index}].poll`);
            expectControlRequest(
                Reflect.get(pollRecord, "request"),
                `${context}[${index}].poll.request`,
            );
            expectResponsePath(
                Reflect.get(pollRecord, "done_path"),
                `${context}[${index}].poll.done_path`,
            );
            expectResponsePath(
                Reflect.get(pollRecord, "error_path"),
                `${context}[${index}].poll.error_path`,
            );
            expectInteger(
                Reflect.get(pollRecord, "interval_ms"),
                `${context}[${index}].poll.interval_ms`,
            );
            expectInteger(
                Reflect.get(pollRecord, "budget_ms"),
                `${context}[${index}].poll.budget_ms`,
            );
        }
    }
}

const FORMATS: ReadonlySet<string> = new Set([
    "openai_response",
    "openai_chat_completion",
    "gemini_generate_content",
    "gemini_interaction",
    "anthropic_messages",
]);

/**
 * One stored definition version: the `(name, version)` identity, the
 * descriptor payload, and the creation timestamp. The payload is authored
 * JSON, validated structurally here; the gateway's write-time boundary owns
 * the semantic rules.
 */
function parseDefinitionResponse(value: unknown, context: string): DefinitionResponse {
    const record = expectRecord(value, context);
    const name = expectString(Reflect.get(record, "name"), `${context}.name`);
    const version = expectInteger(Reflect.get(record, "version"), `${context}.version`);
    const createdAt = expectInteger(Reflect.get(record, "created_at"), `${context}.created_at`);
    const format = expectString(Reflect.get(record, "format"), `${context}.format`);

    if (!FORMATS.has(format)) {
        throw new ContractViolationError(`${context}.format is not an upstream format: ${format}`);
    }

    const inference = expectRecord(Reflect.get(record, "inference"), `${context}.inference`);
    expectStringExpr(Reflect.get(inference, "url"), `${context}.inference.url`);
    expectString(Reflect.get(inference, "upstream_streaming"), `${context}.inference.upstream_streaming`);

    const credential = expectRecord(Reflect.get(record, "credential"), `${context}.credential`);
    const credentialKind = expectString(
        Reflect.get(credential, "type"),
        `${context}.credential.type`,
    );

    if (credentialKind !== "api_key" && credentialKind !== "oauth") {
        throw new ContractViolationError(
            `${context}.credential.type is not a credential kind: ${credentialKind}`,
        );
    }

    if (credentialKind === "oauth") {
        expectString(Reflect.get(credential, "client_id"), `${context}.credential.client_id`);
        expectControlRequest(
            Reflect.get(credential, "grant") === undefined
                ? undefined
                : Reflect.get(expectRecord(Reflect.get(credential, "grant"), `${context}.credential.grant`), "authorization") ?? {},
            `${context}.credential.grant`,
        );
    }

    const provisioning = Reflect.get(record, "provisioning");
    if (provisioning !== undefined && provisioning !== null) {
        expectProvisioning(provisioning, `${context}.provisioning`);
    }

    const discovery = Reflect.get(record, "discovery");
    if (discovery !== undefined && discovery !== null) {
        const discoveryRecord = expectRecord(discovery, `${context}.discovery`);
        expectControlRequest(
            Reflect.get(discoveryRecord, "request"),
            `${context}.discovery.request`,
        );
    }

    return {
        name,
        version,
        format: format as DefinitionResponse["format"],
        // The payload's descriptor sections ride the record verbatim: the
        // structural walk above has checked each section this app renders.
        ...record,
        created_at: createdAt,
    } as DefinitionResponse;
}

export function parseListProviderDefinitionsResponse(
    value: unknown,
): ListProviderDefinitionsResponse {
    const record = expectRecord(value, "response");
    const definitions = expectArray(
        Reflect.get(record, "provider_definitions"),
        "response.provider_definitions",
    );

    return {
        provider_definitions: definitions.map((entry, index) =>
            parseDefinitionResponse(entry, `response.provider_definitions[${index}]`),
        ),
    };
}

export function parseDefinitionEnvelope(value: unknown): DefinitionEnvelope {
    const record = expectRecord(value, "response");

    return {
        provider_definition: parseDefinitionResponse(
            Reflect.get(record, "provider_definition"),
            "response.provider_definition",
        ),
    };
}

/**
 * One finished or polled flow: succeeded carries the provider row, pending
 * carries nothing, failed carries the gateway-owned failure pair.
 */
export function parseCompleteProviderOauthResponse(
    value: unknown,
): CompleteProviderOauthResponse {
    const record = expectRecord(value, "response");
    const status = expectString(Reflect.get(record, "status"), "response.status");

    if (status === "succeeded") {
        return {
            status,
            provider: parseProviderResponse(Reflect.get(record, "provider"), "response.provider"),
        };
    }

    if (status === "pending") {
        return { status };
    }

    if (status === "failed") {
        return {
            status,
            failure_code: expectString(Reflect.get(record, "failure_code"), "response.failure_code"),
            failure_message: expectString(
                Reflect.get(record, "failure_message"),
                "response.failure_message",
            ),
        };
    }

    throw new ContractViolationError(`response.status is not a flow outcome: ${status}`);
}
