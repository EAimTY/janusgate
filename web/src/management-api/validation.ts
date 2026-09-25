import { ContractViolationError } from "/src/management-api/errors";

import {
    type ApiKeyEnvelope,
    type ApiKeyResponse,
    type ApiLogResponse,
    type CompleteProviderOauthResponse,
    type ControlBody,
    type ControlRequest,
    type CreateProviderDefinitionRequest,
    type CredentialAcquisition,
    type DefinitionEnvelope,
    type DefinitionResponse,
    type DiscoverModelsResponse,
    type Discovery,
    type DiscoveryExtract,
    type EmptyResponse,
    ErrorCode,
    type ErrorResponse,
    type Exchange,
    type ExtractionShape,
    type Grant,
    type Inference,
    type JsonValue,
    type ListApiKeysResponse,
    type ListModelsResponse,
    type ListProviderDefinitionsResponse,
    type ListProvidersResponse,
    type ListUsageResponse,
    type ModelEnvelope,
    type ModelResponse,
    type Pagination,
    type ParamRow,
    type Poll,
    type ProviderEnvelope,
    type ProviderResponse,
    type ProvisionExtraction,
    type ProvisioningStep,
    type QueryUsageResponse,
    type RenderOption,
    type ResponseDescriptor,
    type ResponseEncoding,
    type ResponsePath,
    type ResponseTarget,
    type StartProviderOauthResponse,
    type StringExpr,
    type TemplateNode,
    type UpstreamStreaming,
    type UsageBucket,
    type UsageBucketKey,
    type ValueExpr,
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
 * definition payloads (README §5) are the exception — the gateway parses
 * descriptors with `deny_unknown_fields`, so their walk names an undeclared
 * key instead of tolerating it.
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
 * The authored definition payload (README §5): a definition's descriptor as
 * `provider-definitions:create` and `provider-definitions:update` carry it,
 * everything beside the `name`. It is authored JSON — a preset's emission or
 * an operator's edit — and one structural walk over it serves both
 * directions: the creation page validates the edited text before it is sent,
 * and the stored-definition responses build their values through the same
 * walk.
 */
export type DefinitionPayload = Omit<CreateProviderDefinitionRequest, "name">;

/**
 * What validating an authored payload produced: the typed payload, or the
 * offending JSON path — the message the creation page renders inline.
 */
export type DefinitionPayloadResult =
    | {
          readonly payload: DefinitionPayload;
      }
    | {
          readonly error: string;
      };

/**
 * Validates one authored definition payload (README §5): the JSON a preset
 * emitted or an operator edited. The gateway parses descriptors with
 * `deny_unknown_fields` and answers a broken payload with errno 1, so this
 * walk applies the same strictness before the round trip — every closed
 * vocabulary is checked against the generated types, every required key's
 * presence, and every undeclared key is named instead of silently dropped.
 */
export function validateDefinitionPayload(value: unknown): DefinitionPayloadResult {
    try {
        return {
            payload: expectDefinitionPayload(value, "definition"),
        };
    } catch (error) {
        if (error instanceof ContractViolationError) {
            return {
                error: error.message,
            };
        }

        throw error;
    }
}

/**
 * Rejects record keys outside the set the walk consumed: a descriptor field
 * the bindings do not declare would be dropped by the rebuild below, and a
 * dropped field is a mistake to name inline — never one to send a truncated
 * payload about (the route payloads above tolerate extras instead, because
 * those bodies are never rebuilt).
 */
function expectDeclaredKeys(
    record: object,
    declared: ReadonlyArray<string>,
    context: string,
): void {
    for (const key of Object.keys(record)) {
        if (!declared.includes(key)) {
            throw new ContractViolationError(`${context}.${key} is not a declared field`);
        }
    }
}

/** Whether an optional-and-nullable descriptor value carries content. */
function isPresentValue(value: unknown): boolean {
    return value !== undefined && value !== null;
}

/**
 * One authored definition payload, rebuilt field by field. The context is the
 * JSON path the messages name, `"definition"` at the top of an authored
 * payload and the response path inside a stored-definition parse.
 */
function expectDefinitionPayload(value: unknown, context: string): DefinitionPayload {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "format",
            "inference",
            "credential",
            "provisioning",
            "discovery",
        ],
        context,
    );

    const payload: DefinitionPayload = {
        format: parseUpstreamFormat(Reflect.get(record, "format"), `${context}.format`),
        inference: expectInference(Reflect.get(record, "inference"), `${context}.inference`),
        credential: expectCredential(Reflect.get(record, "credential"), `${context}.credential`),
    };

    const provisioning = Reflect.get(record, "provisioning");

    if (isPresentValue(provisioning)) {
        payload.provisioning = expectProvisioning(provisioning, `${context}.provisioning`);
    }

    const discovery = Reflect.get(record, "discovery");

    if (isPresentValue(discovery)) {
        payload.discovery = expectDiscovery(discovery, `${context}.discovery`);
    }

    return payload;
}

/** The upstream streaming mode a definition pins (README §5.2). */
function expectUpstreamStreaming(value: unknown, context: string): UpstreamStreaming {
    if (value === "always" || value === "match_client") {
        return value;
    }

    throw new ContractViolationError(
        `${context} is not an upstream streaming mode: ${String(value)}`,
    );
}

/** The closed render-option vocabulary of the format codecs (README §5.2). */
function expectRenderOption(value: unknown, context: string): RenderOption {
    if (value === "omit_max_output_tokens") {
        return value;
    }

    throw new ContractViolationError(`${context} is not a render option: ${String(value)}`);
}

/** The wire encoding a control response arrives in (README §5.1). */
function expectResponseEncoding(value: unknown, context: string): ResponseEncoding {
    if (value === "json" || value === "form") {
        return value;
    }

    throw new ContractViolationError(`${context} is not a response encoding: ${String(value)}`);
}

/**
 * The inference request descriptor (README §5.2), rebuilt with every optional
 * section the operator supplied and `null` normalized to absence.
 */
function expectInference(value: unknown, context: string): Inference {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "url",
            "stream_url",
            "headers",
            "upstream_streaming",
            "render_options",
            "envelope",
            "response_unwrap",
        ],
        context,
    );

    const inference: Inference = {
        url: expectStringExpr(Reflect.get(record, "url"), `${context}.url`),
        upstream_streaming: expectUpstreamStreaming(
            Reflect.get(record, "upstream_streaming"),
            `${context}.upstream_streaming`,
        ),
    };

    const streamUrl = Reflect.get(record, "stream_url");

    if (isPresentValue(streamUrl)) {
        inference.stream_url = expectStringExpr(streamUrl, `${context}.stream_url`);
    }

    const headers = Reflect.get(record, "headers");

    if (headers !== undefined) {
        inference.headers = expectParamRows(headers, `${context}.headers`);
    }

    const renderOptions = Reflect.get(record, "render_options");

    if (renderOptions !== undefined) {
        inference.render_options = expectArray(renderOptions, `${context}.render_options`).map(
            (option, index) => expectRenderOption(option, `${context}.render_options[${index}]`),
        );
    }

    const envelope = Reflect.get(record, "envelope");

    if (isPresentValue(envelope)) {
        inference.envelope = expectTemplateNode(envelope, `${context}.envelope`);
    }

    const responseUnwrap = Reflect.get(record, "response_unwrap");

    if (isPresentValue(responseUnwrap)) {
        inference.response_unwrap = expectString(responseUnwrap, `${context}.response_unwrap`);
    }

    return inference;
}

/** One value expression part, rebuilt from its validated kind (README §5.1). */
function expectExpressionPart(part: unknown, context: string): ValueExpr {
    const record = expectRecord(part, context);
    const kind = expectString(Reflect.get(record, "type"), `${context}.type`);

    switch (kind) {
        case "literal":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "value",
                ],
                context,
            );
            return {
                type: "literal",
                value: expectString(Reflect.get(record, "value"), `${context}.value`),
            };
        case "secret":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "secret",
            };
        case "slot":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "name",
                ],
                context,
            );
            return {
                type: "slot",
                name: expectString(Reflect.get(record, "name"), `${context}.name`),
            };
        case "client_id":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "client_id",
            };
        case "client_secret":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "client_secret",
            };
        case "authorization_code":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "authorization_code",
            };
        case "code_verifier":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "code_verifier",
            };
        case "redirect_uri":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "redirect_uri",
            };
        case "oauth_state":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "oauth_state",
            };
        case "device_code":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "device_code",
            };
        case "refresh_token":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "refresh_token",
            };
        case "request_uuid":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "request_uuid",
            };
        case "model":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "model",
            };
        case "base":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "base",
            };
        case "coalesce":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "parts",
                ],
                context,
            );
            return {
                type: "coalesce",
                parts: expectExpressionParts(Reflect.get(record, "parts"), `${context}.parts`),
            };
        default:
            throw new ContractViolationError(`${context}.type is not an expression kind: ${kind}`);
    }
}

/** Validates one string expression: an ordered array of expression parts. */
function expectStringExpr(value: unknown, context: string): StringExpr {
    return expectArray(value, context).map((part, index) =>
        expectExpressionPart(part, `${context}[${index}]`),
    );
}

/** The parts of one string expression, as the expression walkers consume them. */
function expectExpressionParts(value: unknown, context: string): Array<ValueExpr> {
    return expectStringExpr(value, context);
}

/** One param row: a name and its expression, rebuilt (README §5.1). */
function expectParamRows(value: unknown, context: string): Array<ParamRow> {
    return expectArray(value, context).map((row, index) => {
        const record = expectRecord(row, `${context}[${index}]`);
        expectDeclaredKeys(
            record,
            [
                "name",
                "value",
            ],
            `${context}[${index}]`,
        );

        return {
            name: expectString(Reflect.get(record, "name"), `${context}[${index}].name`),
            value: expectStringExpr(Reflect.get(record, "value"), `${context}[${index}].value`),
        };
    });
}

/** One JSON value, walked so a non-JSON leaf (a function, a bare `undefined`) is named. */
function expectJsonValue(value: unknown, context: string): JsonValue {
    if (value === null) {
        return null;
    }

    switch (typeof value) {
        case "string":
            return value;
        case "number":
            if (Number.isFinite(value)) {
                return value;
            }

            throw new ContractViolationError(`${context} is not a JSON value`);
        case "boolean":
            return value;
        case "object":
            if (Array.isArray(value)) {
                return value.map((entry, index) => expectJsonValue(entry, `${context}[${index}]`));
            }

            return Object.fromEntries(
                Object.keys(value).map((key) => [
                    key,
                    expectJsonValue(Reflect.get(value, key), `${context}.${key}`),
                ]),
            );
        default:
            throw new ContractViolationError(`${context} is not a JSON value`);
    }
}

/** One template tree node, rebuilt from its node kind (README §5.1). */
function expectTemplateNode(value: unknown, context: string): TemplateNode {
    const record = expectRecord(value, context);
    const kind = expectString(Reflect.get(record, "type"), `${context}.type`);

    switch (kind) {
        case "json":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "value",
                ],
                context,
            );
            return {
                type: "json",
                value: expectJsonValue(Reflect.get(record, "value"), `${context}.value`),
            };

        case "string":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "expr",
                ],
                context,
            );
            return {
                type: "string",
                expr: expectStringExpr(Reflect.get(record, "expr"), `${context}.expr`),
            };

        case "body":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "body",
            };

        case "object": {
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "fields",
                ],
                context,
            );
            const fields = expectRecord(Reflect.get(record, "fields"), `${context}.fields`);

            return {
                type: "object",
                fields: Object.fromEntries(
                    Object.keys(fields).map((name) => [
                        name,
                        expectTemplateNode(Reflect.get(fields, name), `${context}.fields.${name}`),
                    ]),
                ),
            };
        }

        case "array":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "items",
                ],
                context,
            );
            return {
                type: "array",
                items: expectArray(Reflect.get(record, "items"), `${context}.items`).map(
                    (item, index) => expectTemplateNode(item, `${context}.items[${index}]`),
                ),
            };

        default:
            throw new ContractViolationError(`${context}.type is not a template node: ${kind}`);
    }
}

/** One control body: a form-encoded field set or a typed JSON tree (README §5.1). */
function expectControlBody(value: unknown, context: string): ControlBody {
    const record = expectRecord(value, context);
    const kind = expectString(Reflect.get(record, "kind"), `${context}.kind`);

    if (kind === "form") {
        expectDeclaredKeys(
            record,
            [
                "kind",
                "fields",
            ],
            context,
        );

        return {
            kind: "form",
            fields: expectParamRows(Reflect.get(record, "fields"), `${context}.fields`),
        };
    }

    if (kind === "json") {
        expectDeclaredKeys(
            record,
            [
                "kind",
                "node",
            ],
            context,
        );

        return {
            kind: "json",
            node: expectTemplateNode(Reflect.get(record, "node"), `${context}.node`),
        };
    }

    throw new ContractViolationError(`${context}.kind is not a control body: ${kind}`);
}

/** One control request: method, URL, headers, query, and body (README §5.1). */
function expectControlRequest(value: unknown, context: string): ControlRequest {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "method",
            "url",
            "headers",
            "query",
            "body",
        ],
        context,
    );

    const method = expectString(Reflect.get(record, "method"), `${context}.method`);

    if (method !== "GET" && method !== "POST") {
        throw new ContractViolationError(`${context}.method is not a control method: ${method}`);
    }

    const request: ControlRequest = {
        method,
        url: expectStringExpr(Reflect.get(record, "url"), `${context}.url`),
    };

    const headers = Reflect.get(record, "headers");

    if (headers !== undefined) {
        request.headers = expectParamRows(headers, `${context}.headers`);
    }

    const query = Reflect.get(record, "query");

    if (query !== undefined) {
        request.query = expectParamRows(query, `${context}.query`);
    }

    const body = Reflect.get(record, "body");

    if (isPresentValue(body)) {
        request.body = expectControlBody(body, `${context}.body`);
    }

    return request;
}

/** One response path: the ordered member chain (README §5.1). */
function expectResponsePath(value: unknown, context: string): ResponsePath {
    return expectArray(value, context).map((member, index) =>
        expectString(member, `${context}[${index}]`),
    );
}

/** One merged working-state target, rebuilt from its tag (README §5.3, §5.4). */
function expectResponseTarget(value: unknown, context: string): ResponseTarget {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (type === "slot") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "name",
            ],
            context,
        );

        return {
            type: "slot",
            name: expectString(Reflect.get(record, "name"), `${context}.name`),
        };
    }

    switch (type) {
        case "access_token":
        case "refresh_token":
        case "expires_in_seconds":
        case "id_token":
        case "base_url":
        case "device_code":
        case "user_code":
        case "verification_uri":
        case "verification_uri_complete":
        case "interval_seconds":
        case "expires_at_unix_seconds":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type,
            };
        default:
            throw new ContractViolationError(`${context}.type is not a response target: ${type}`);
    }
}

/** One response mapping field: the path, its target, and the required flag. */
function expectResponseField(
    value: unknown,
    context: string,
): {
    path: ResponsePath;
    target: ResponseTarget;
    required: boolean;
} {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "path",
            "target",
            "required",
        ],
        context,
    );

    return {
        path: expectResponsePath(Reflect.get(record, "path"), `${context}.path`),
        target: expectResponseTarget(Reflect.get(record, "target"), `${context}.target`),
        required: expectBoolean(Reflect.get(record, "required"), `${context}.required`),
    };
}

/** One response descriptor: the encoding and its field mappings (README §5.1). */
function expectResponseDescriptor(value: unknown, context: string): ResponseDescriptor {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "encoding",
            "fields",
        ],
        context,
    );

    return {
        encoding: expectResponseEncoding(Reflect.get(record, "encoding"), `${context}.encoding`),
        fields: expectArray(Reflect.get(record, "fields"), `${context}.fields`).map(
            (field, index) => expectResponseField(field, `${context}.fields[${index}]`),
        ),
    };
}

/** The declared shape of one extracted value (README §5.4). */
function expectExtractionShape(value: unknown, context: string): ExtractionShape {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (type === "string" || type === "integer") {
        expectDeclaredKeys(
            record,
            [
                "type",
            ],
            context,
        );
        return {
            type,
        };
    }

    if (type === "string_or_field") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "field",
            ],
            context,
        );

        return {
            type: "string_or_field",
            field: expectString(Reflect.get(record, "field"), `${context}.field`),
        };
    }

    throw new ContractViolationError(`${context}.type is not an extraction shape: ${type}`);
}

/** One provisioning extraction: the path, its target, the shape, and the flag. */
function expectProvisionExtraction(value: unknown, context: string): ProvisionExtraction {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "path",
            "target",
            "shape",
            "required",
        ],
        context,
    );

    return {
        path: expectResponsePath(Reflect.get(record, "path"), `${context}.path`),
        target: expectResponseTarget(Reflect.get(record, "target"), `${context}.target`),
        shape: expectExtractionShape(Reflect.get(record, "shape"), `${context}.shape`),
        required: expectBoolean(Reflect.get(record, "required"), `${context}.required`),
    };
}

/** The extractions of one provisioning step or poll, when the key is present. */
function expectProvisionExtractions(value: unknown, context: string): Array<ProvisionExtraction> {
    return expectArray(value, context).map((entry, index) =>
        expectProvisionExtraction(entry, `${context}[${index}]`),
    );
}

/** The long-running operation poll of a provisioning step (README §5.4). */
function expectPoll(value: unknown, context: string): Poll {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "request",
            "response_encoding",
            "done_path",
            "error_path",
            "result_extract",
            "interval_ms",
            "budget_ms",
        ],
        context,
    );

    const poll: Poll = {
        request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
        response_encoding: expectResponseEncoding(
            Reflect.get(record, "response_encoding"),
            `${context}.response_encoding`,
        ),
        done_path: expectResponsePath(Reflect.get(record, "done_path"), `${context}.done_path`),
        error_path: expectResponsePath(Reflect.get(record, "error_path"), `${context}.error_path`),
        interval_ms: expectInteger(Reflect.get(record, "interval_ms"), `${context}.interval_ms`),
        budget_ms: expectInteger(Reflect.get(record, "budget_ms"), `${context}.budget_ms`),
    };

    const resultExtract = Reflect.get(record, "result_extract");

    if (resultExtract !== undefined) {
        poll.result_extract = expectProvisionExtractions(
            resultExtract,
            `${context}.result_extract`,
        );
    }

    return poll;
}

/** One provisioning step, rebuilt with its guard, extractions, and poll (README §5.4). */
function expectProvisioningStep(value: unknown, context: string): ProvisioningStep {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "request",
            "response_encoding",
            "extract",
            "skip_if",
            "poll",
        ],
        context,
    );

    const step: ProvisioningStep = {
        request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
        response_encoding: expectResponseEncoding(
            Reflect.get(record, "response_encoding"),
            `${context}.response_encoding`,
        ),
    };

    const extract = Reflect.get(record, "extract");

    if (extract !== undefined) {
        step.extract = expectProvisionExtractions(extract, `${context}.extract`);
    }

    const skipIf = Reflect.get(record, "skip_if");

    if (isPresentValue(skipIf)) {
        step.skip_if = expectString(skipIf, `${context}.skip_if`);
    }

    const poll = Reflect.get(record, "poll");

    if (isPresentValue(poll)) {
        step.poll = expectPoll(poll, `${context}.poll`);
    }

    return step;
}

/** Validates one provisioning sequence with its polls (README §5.4). */
function expectProvisioning(value: unknown, context: string): Array<ProvisioningStep> {
    return expectArray(value, context).map((step, index) =>
        expectProvisioningStep(step, `${context}[${index}]`),
    );
}

/** The page-list extraction of a discovery, rebuilt from its mode (README §5.5). */
function expectDiscoveryExtract(value: unknown, context: string): DiscoveryExtract {
    const record = expectRecord(value, context);
    const mode = expectString(Reflect.get(record, "mode"), `${context}.mode`);

    if (mode === "array") {
        expectDeclaredKeys(
            record,
            [
                "mode",
                "array_at",
                "id_field",
                "strip_prefix",
            ],
            context,
        );

        const extract: DiscoveryExtract = {
            mode: "array",
            array_at: expectResponsePath(Reflect.get(record, "array_at"), `${context}.array_at`),
            id_field: expectResponsePath(Reflect.get(record, "id_field"), `${context}.id_field`),
        };

        const stripPrefix = Reflect.get(record, "strip_prefix");

        if (isPresentValue(stripPrefix)) {
            extract.strip_prefix = expectString(stripPrefix, `${context}.strip_prefix`);
        }

        return extract;
    }

    if (mode === "map_keys") {
        expectDeclaredKeys(
            record,
            [
                "mode",
                "at",
            ],
            context,
        );

        return {
            mode: "map_keys",
            at: expectResponsePath(Reflect.get(record, "at"), `${context}.at`),
        };
    }

    throw new ContractViolationError(`${context}.mode is not a discovery extraction: ${mode}`);
}

/** The pagination loop of a discovery, rebuilt from its mode (README §5.5). */
function expectPagination(value: unknown, context: string): Pagination {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (type === "none") {
        expectDeclaredKeys(
            record,
            [
                "type",
            ],
            context,
        );
        return {
            type: "none",
        };
    }

    if (type === "token") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "request_param",
                "response_path",
                "max_pages",
            ],
            context,
        );

        return {
            type: "token",
            request_param: expectString(
                Reflect.get(record, "request_param"),
                `${context}.request_param`,
            ),
            response_path: expectResponsePath(
                Reflect.get(record, "response_path"),
                `${context}.response_path`,
            ),
            max_pages: expectInteger(Reflect.get(record, "max_pages"), `${context}.max_pages`),
        };
    }

    if (type === "cursor") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "has_more_path",
                "last_id_path",
                "request_param",
                "max_pages",
            ],
            context,
        );

        return {
            type: "cursor",
            has_more_path: expectResponsePath(
                Reflect.get(record, "has_more_path"),
                `${context}.has_more_path`,
            ),
            last_id_path: expectResponsePath(
                Reflect.get(record, "last_id_path"),
                `${context}.last_id_path`,
            ),
            request_param: expectString(
                Reflect.get(record, "request_param"),
                `${context}.request_param`,
            ),
            max_pages: expectInteger(Reflect.get(record, "max_pages"), `${context}.max_pages`),
        };
    }

    throw new ContractViolationError(`${context}.type is not a pagination mode: ${type}`);
}

/** The upstream model-list discovery of a definition (README §5.5). */
function expectDiscovery(value: unknown, context: string): Discovery {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "request",
            "extract",
            "pagination",
        ],
        context,
    );

    const discovery: Discovery = {
        request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
        extract: expectDiscoveryExtract(Reflect.get(record, "extract"), `${context}.extract`),
    };

    const pagination = Reflect.get(record, "pagination");

    if (isPresentValue(pagination)) {
        discovery.pagination = expectPagination(pagination, `${context}.pagination`);
    }

    return discovery;
}

/** One durable slot initializer, rebuilt (README §5.3). */
function expectSlotInitializer(
    value: unknown,
    context: string,
): {
    name: string;
    generator: "uuid" | "uuid_hex";
} {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "name",
            "generator",
        ],
        context,
    );
    const generator = expectString(Reflect.get(record, "generator"), `${context}.generator`);

    if (generator !== "uuid" && generator !== "uuid_hex") {
        throw new ContractViolationError(
            `${context}.generator is not a slot generator: ${generator}`,
        );
    }

    return {
        name: expectString(Reflect.get(record, "name"), `${context}.name`),
        generator,
    };
}

/** The OAuth authorization grant a flow runs, rebuilt from its kind (README §5.3). */
function expectGrant(value: unknown, context: string): Grant {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (type === "authorization_code") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "authorize_url",
                "redirect_uri",
                "pkce",
                "extra_params",
            ],
            context,
        );

        const pkce = expectString(Reflect.get(record, "pkce"), `${context}.pkce`);

        if (pkce !== "none" && pkce !== "s256") {
            throw new ContractViolationError(`${context}.pkce is not a PKCE mode: ${pkce}`);
        }

        const grant: Grant = {
            type: "authorization_code",
            authorize_url: expectStringExpr(
                Reflect.get(record, "authorize_url"),
                `${context}.authorize_url`,
            ),
            redirect_uri: expectString(
                Reflect.get(record, "redirect_uri"),
                `${context}.redirect_uri`,
            ),
            pkce,
        };

        const extraParams = Reflect.get(record, "extra_params");

        if (extraParams !== undefined) {
            grant.extra_params = expectParamRows(extraParams, `${context}.extra_params`);
        }

        return grant;
    }

    if (type === "device") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "authorization",
                "expires_in_fallback_seconds",
            ],
            context,
        );

        const authorizationRecord = expectRecord(
            Reflect.get(record, "authorization"),
            `${context}.authorization`,
        );
        expectDeclaredKeys(
            authorizationRecord,
            [
                "request",
                "response",
            ],
            `${context}.authorization`,
        );

        const grant: Grant = {
            type: "device",
            authorization: {
                request: expectControlRequest(
                    Reflect.get(authorizationRecord, "request"),
                    `${context}.authorization.request`,
                ),
                response: expectResponseDescriptor(
                    Reflect.get(authorizationRecord, "response"),
                    `${context}.authorization.response`,
                ),
            },
        };

        const fallback = Reflect.get(record, "expires_in_fallback_seconds");

        if (isPresentValue(fallback)) {
            grant.expires_in_fallback_seconds = expectInteger(
                fallback,
                `${context}.expires_in_fallback_seconds`,
            );
        }

        return grant;
    }

    throw new ContractViolationError(`${context}.type is not a grant kind: ${type}`);
}

/** The token exchange redeeming a grant's product (README §5.3). */
function expectExchange(value: unknown, context: string): Exchange {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "request",
            "response",
            "terminal_codes",
        ],
        context,
    );

    return {
        request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
        response: expectResponseDescriptor(Reflect.get(record, "response"), `${context}.response`),
        terminal_codes: expectArray(
            Reflect.get(record, "terminal_codes"),
            `${context}.terminal_codes`,
        ).map((code, index) => expectString(code, `${context}.terminal_codes[${index}]`)),
    };
}

/** How an OAuth-acquired credential renews, rebuilt from its mode (README §5.3). */
function expectTokenRefresh(
    value: unknown,
    context: string,
):
    | {
          type: "grant";
          request: ControlRequest;
          response?: ResponseDescriptor | null;
          terminal_codes: Array<string>;
      }
    | {
          type: "reprovision";
      }
    | {
          type: "none";
      } {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (type === "grant") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "request",
                "response",
                "terminal_codes",
            ],
            context,
        );

        const refresh: {
            type: "grant";
            request: ControlRequest;
            response?: ResponseDescriptor | null;
            terminal_codes: Array<string>;
        } = {
            type: "grant",
            request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
            terminal_codes: expectArray(
                Reflect.get(record, "terminal_codes"),
                `${context}.terminal_codes`,
            ).map((code, index) => expectString(code, `${context}.terminal_codes[${index}]`)),
        };

        const response = Reflect.get(record, "response");

        if (isPresentValue(response)) {
            refresh.response = expectResponseDescriptor(response, `${context}.response`);
        }

        return refresh;
    }

    if (type === "reprovision" || type === "none") {
        expectDeclaredKeys(
            record,
            [
                "type",
            ],
            context,
        );
        return {
            type,
        };
    }

    throw new ContractViolationError(`${context}.type is not a refresh mode: ${type}`);
}

/**
 * The credential acquisition of a definition, rebuilt from its kind
 * (README §5.3): an `api_key` credential is its tag alone, an `oauth`
 * credential the full recipe.
 */
function expectCredential(value: unknown, context: string): CredentialAcquisition {
    const record = expectRecord(value, context);
    const kind = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (kind === "api_key") {
        expectDeclaredKeys(
            record,
            [
                "type",
            ],
            context,
        );
        return {
            type: "api_key",
        };
    }

    if (kind === "oauth") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "client_id",
                "client_secret",
                "access_token_lifetime_seconds",
                "initial_slots",
                "grant",
                "exchange",
                "id_token_claims",
                "refresh",
            ],
            context,
        );

        const oauth: Extract<
            CredentialAcquisition,
            {
                type: "oauth";
            }
        > = {
            type: "oauth",
            client_id: expectString(Reflect.get(record, "client_id"), `${context}.client_id`),
            grant: expectGrant(Reflect.get(record, "grant"), `${context}.grant`),
            exchange: expectExchange(Reflect.get(record, "exchange"), `${context}.exchange`),
            refresh: expectTokenRefresh(Reflect.get(record, "refresh"), `${context}.refresh`),
        };

        const clientSecret = Reflect.get(record, "client_secret");

        if (isPresentValue(clientSecret)) {
            oauth.client_secret = expectString(clientSecret, `${context}.client_secret`);
        }

        const lifetime = Reflect.get(record, "access_token_lifetime_seconds");

        if (isPresentValue(lifetime)) {
            oauth.access_token_lifetime_seconds = expectInteger(
                lifetime,
                `${context}.access_token_lifetime_seconds`,
            );
        }

        const initialSlots = Reflect.get(record, "initial_slots");

        if (initialSlots !== undefined) {
            oauth.initial_slots = expectArray(initialSlots, `${context}.initial_slots`).map(
                (slot, index) => expectSlotInitializer(slot, `${context}.initial_slots[${index}]`),
            );
        }

        const idTokenClaims = Reflect.get(record, "id_token_claims");

        if (idTokenClaims !== undefined) {
            oauth.id_token_claims = expectArray(idTokenClaims, `${context}.id_token_claims`).map(
                (claim, index) => {
                    const claimRecord = expectRecord(claim, `${context}.id_token_claims[${index}]`);
                    expectDeclaredKeys(
                        claimRecord,
                        [
                            "claim",
                            "slot",
                        ],
                        `${context}.id_token_claims[${index}]`,
                    );

                    return {
                        claim: expectResponsePath(
                            Reflect.get(claimRecord, "claim"),
                            `${context}.id_token_claims[${index}].claim`,
                        ),
                        slot: expectString(
                            Reflect.get(claimRecord, "slot"),
                            `${context}.id_token_claims[${index}].slot`,
                        ),
                    };
                },
            );
        }

        return oauth;
    }

    throw new ContractViolationError(`${context}.type is not a credential kind: ${kind}`);
}

/**
 * One stored definition version: the `(name, version)` identity and the
 * creation timestamp — the fields the app reads — built beside the payload
 * the shared walk above reconstructs, so the value is assembled from
 * validated fields and never asserted.
 */
function parseDefinitionResponse(value: unknown, context: string): DefinitionResponse {
    const record = expectRecord(value, context);

    const payload = expectDefinitionPayload(
        {
            format: Reflect.get(record, "format"),
            inference: Reflect.get(record, "inference"),
            credential: Reflect.get(record, "credential"),
            provisioning: Reflect.get(record, "provisioning"),
            discovery: Reflect.get(record, "discovery"),
        },
        context,
    );

    return {
        name: expectString(Reflect.get(record, "name"), `${context}.name`),
        version: expectInteger(Reflect.get(record, "version"), `${context}.version`),
        created_at: expectInteger(Reflect.get(record, "created_at"), `${context}.created_at`),
        ...payload,
    };
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
