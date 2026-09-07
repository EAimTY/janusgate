import {
    OAUTH_FLOW_TYPES,
    type OAuthFlowType,
    PROVIDER_TYPES,
    type ProviderType,
    UPSTREAM_PROTOCOLS,
    type UpstreamProtocol,
} from "/common/domain";
import { ValidationError } from "/common/errors";
import {
    CANONICAL_THINKING_LEVELS,
    MILLISECONDS_PER_SECOND,
    MODEL_PREFIX_MAX_LENGTH,
    MODEL_PREFIX_PATTERN,
    PROVIDER_NAME_MAX_LENGTH,
    PROVIDER_NAME_PATTERN,
    type ThinkingLevel,
} from "/config";

export function isJsonObject(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function requireJsonObject(value: unknown, fieldName = "value"): Record<string, unknown> {
    if (!isJsonObject(value)) {
        throw new ValidationError(`${fieldName} must be a JSON object`);
    }
    return value;
}

export function requireStringField(value: Record<string, unknown>, fieldName: string): string {
    const field = value[fieldName];
    if (typeof field !== "string") {
        throw new ValidationError(`${fieldName} must be a string`);
    }
    return field;
}

export function parseProviderName(value: unknown): string {
    if (typeof value !== "string" || !PROVIDER_NAME_PATTERN.test(value)) {
        throw new ValidationError(`name must match [a-z0-9_-]{1,${PROVIDER_NAME_MAX_LENGTH}}`);
    }
    return value;
}

export function parseModelPrefix(value: unknown): string {
    if (typeof value !== "string" || !MODEL_PREFIX_PATTERN.test(value)) {
        throw new ValidationError(
            `modelPrefix must contain at most ${MODEL_PREFIX_MAX_LENGTH} characters from [a-z0-9._:/-]`,
        );
    }
    return value;
}

export function parseNonEmptyString(value: unknown, fieldName: string): string {
    if (typeof value !== "string" || value.length === 0) {
        throw new ValidationError(`${fieldName} must be a non-empty string`);
    }
    return value;
}

export function parseProviderType(value: unknown): ProviderType {
    for (const providerType of PROVIDER_TYPES) {
        if (value === providerType) {
            return providerType;
        }
    }
    throw new ValidationError(`type must be one of: ${PROVIDER_TYPES.join(", ")}`);
}

export function parseOAuthFlowType(value: unknown): OAuthFlowType {
    for (const flowType of OAUTH_FLOW_TYPES) {
        if (value === flowType) {
            return flowType;
        }
    }
    throw new ValidationError(`type must be one of: ${OAUTH_FLOW_TYPES.join(", ")}`);
}

export function parseUpstreamProtocol(value: unknown): UpstreamProtocol {
    for (const protocol of UPSTREAM_PROTOCOLS) {
        if (value === protocol) {
            return protocol;
        }
    }
    throw new ValidationError(`protocol must be one of: ${UPSTREAM_PROTOCOLS.join(", ")}`);
}

export function parseBaseUrl(value: unknown): string {
    if (typeof value !== "string") {
        throw new ValidationError("baseUrl must be an HTTP(S) URL");
    }

    let parsed: URL;
    try {
        parsed = new URL(value);
    } catch {
        throw new ValidationError("baseUrl must be an HTTP(S) URL");
    }
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
        throw new ValidationError("baseUrl must be an HTTP(S) URL");
    }

    return parsed.href.replace(/\/+$/, "");
}

function parseThinkingLevel(value: unknown): ThinkingLevel {
    for (const level of CANONICAL_THINKING_LEVELS) {
        if (value === level) {
            return level;
        }
    }
    throw new ValidationError(
        `thinking level must be one of: ${CANONICAL_THINKING_LEVELS.join(", ")}`,
    );
}

export function parseThinkingLevels(value: unknown): readonly ThinkingLevel[] | null {
    if (value === null) {
        return null;
    }
    if (!Array.isArray(value)) {
        throw new ValidationError("thinkingLevels must be an array or null");
    }

    const levels = value.map(parseThinkingLevel);
    if (new Set(levels).size !== levels.length) {
        throw new ValidationError("thinkingLevels must not contain duplicates");
    }
    return CANONICAL_THINKING_LEVELS.filter((level) => levels.includes(level));
}

export function isThinkingLevel(value: unknown): value is ThinkingLevel {
    return CANONICAL_THINKING_LEVELS.some((level) => value === level);
}

/** Single parser for the stored `models.thinking_levels` JSON column; contract violations are fatal. */
export function parseStoredThinkingLevels(json: string | null): readonly ThinkingLevel[] | null {
    if (json === null) return null;
    let value: unknown;
    try {
        value = JSON.parse(json);
    } catch {
        throw new Error("Invalid stored thinking levels");
    }
    if (!Array.isArray(value) || !value.every(isThinkingLevel)) {
        throw new Error("Invalid stored thinking levels");
    }
    return value;
}

export function composeModelName(prefix: string, base: string): string {
    if (base.length === 0) {
        throw new ValidationError("model base name must be non-empty");
    }
    return `${prefix}${base}`;
}

export function unixSeconds(nowMilliseconds = Date.now()): number {
    return Math.floor(nowMilliseconds / MILLISECONDS_PER_SECOND);
}
