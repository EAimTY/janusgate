import { ContractViolationError } from "/src/management-api/errors";

import {
    type ApiKeyEnvelope,
    type ApiKeyResponse,
    type DiscoverModelsResponse,
    type EmptyResponse,
    ErrorCode,
    type ErrorResponse,
    type ListApiKeysResponse,
    type ListModelsResponse,
    type ListProvidersResponse,
    type ModelEnvelope,
    type ModelResponse,
    type ProviderEnvelope,
    type ProviderResponse,
    type ReauthorizeProviderResponse,
    type StartProviderOauthResponse,
    type ThinkingLevel,
} from "/src/management-api/generated/v0";

import { parseThinkingLevel } from "/src/management-api/vocabulary";

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

function expectThinkingLevelsOverride(value: unknown, context: string): ThinkingLevel[] | null {
    if (value === null) {
        return null;
    }

    const entries = expectArray(value, context);
    return entries.map((entry, index) => parseThinkingLevel(entry, `${context}[${index}]`));
}

/** One provider row; the union is discriminated by the restated `type`. */
export function parseProviderResponse(value: unknown, context: string): ProviderResponse {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);
    const id = expectInteger(Reflect.get(record, "id"), `${context}.id`);
    const name = expectString(Reflect.get(record, "name"), `${context}.name`);

    const modelPrefix = expectString(
        Reflect.get(record, "model_prefix"),
        `${context}.model_prefix`,
    );

    const isEnabled = expectBoolean(Reflect.get(record, "is_enabled"), `${context}.is_enabled`);

    // A closure: the static-key variants have no `expires_at`, so its absence is
    // only an error where the union variant declares the field.
    const readExpiresAt = () =>
        expectNullableInteger(Reflect.get(record, "expires_at"), `${context}.expires_at`);

    switch (type) {
        case "openai_response":
            return {
                type: "openai_response",
                id,
                name,
                model_prefix: modelPrefix,
                is_enabled: isEnabled,
                base_url: expectString(Reflect.get(record, "base_url"), `${context}.base_url`),
            };

        case "openai_chat_completion":
            return {
                type: "openai_chat_completion",
                id,
                name,
                model_prefix: modelPrefix,
                is_enabled: isEnabled,
                base_url: expectString(Reflect.get(record, "base_url"), `${context}.base_url`),
            };

        case "gemini_generate_content":
            return {
                type: "gemini_generate_content",
                id,
                name,
                model_prefix: modelPrefix,
                is_enabled: isEnabled,
                base_url: expectString(Reflect.get(record, "base_url"), `${context}.base_url`),
            };

        case "codex_oauth":
            return {
                type: "codex_oauth",
                id,
                name,
                model_prefix: modelPrefix,
                is_enabled: isEnabled,
                expires_at: readExpiresAt(),
            };

        case "antigravity_oauth":
            return {
                type: "antigravity_oauth",
                id,
                name,
                model_prefix: modelPrefix,
                is_enabled: isEnabled,
                expires_at: readExpiresAt(),
            };

        default:
            throw new ContractViolationError(
                `${context}.type is not a known provider type: ${type}`,
            );
    }
}

/** One model row, including the two nullable mirror columns. */
export function parseModelResponse(value: unknown, context: string): ModelResponse {
    const record = expectRecord(value, context);

    return {
        provider_id: expectInteger(Reflect.get(record, "provider_id"), `${context}.provider_id`),
        provider: expectString(Reflect.get(record, "provider"), `${context}.provider`),
        upstream_id: expectString(Reflect.get(record, "upstream_id"), `${context}.upstream_id`),
        id: expectString(Reflect.get(record, "id"), `${context}.id`),
        id_override: expectNullableString(
            Reflect.get(record, "id_override"),
            `${context}.id_override`,
        ),
        is_enabled: expectBoolean(Reflect.get(record, "is_enabled"), `${context}.is_enabled`),
        is_effectively_enabled: expectBoolean(
            Reflect.get(record, "is_effectively_enabled"),
            `${context}.is_effectively_enabled`,
        ),
        thinking_levels_override: expectThinkingLevelsOverride(
            Reflect.get(record, "thinking_levels_override"),
            `${context}.thinking_levels_override`,
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

/** One gateway API key row: the complete readable state, plaintext secret included. */
export function parseApiKeyResponse(value: unknown, context: string): ApiKeyResponse {
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

export function parseStartProviderOauthResponse(value: unknown): StartProviderOauthResponse {
    const record = expectRecord(value, "response");

    return {
        authorize_url: expectString(Reflect.get(record, "authorize_url"), "response.authorize_url"),
        state: expectString(Reflect.get(record, "state"), "response.state"),
        expires_at: expectInteger(Reflect.get(record, "expires_at"), "response.expires_at"),
    };
}

export function parseReauthorizeProviderResponse(value: unknown): ReauthorizeProviderResponse {
    const record = expectRecord(value, "response");

    return {
        expires_at: expectInteger(Reflect.get(record, "expires_at"), "response.expires_at"),
    };
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
