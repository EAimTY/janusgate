import { isJsonObject } from "/common/validation";
import { TranslationError } from "./errors";

type JsonPrimitive = boolean | number | string | null;
export type JsonValue =
    | JsonPrimitive
    | JsonValue[]
    | {
          [key: string]: JsonValue;
      };
export type JsonObject = {
    [key: string]: JsonValue;
};

export function isJsonValue(value: unknown): value is JsonValue {
    if (
        value === null ||
        typeof value === "boolean" ||
        typeof value === "string" ||
        (typeof value === "number" && Number.isFinite(value))
    ) {
        return true;
    }
    if (Array.isArray(value)) {
        return value.every(isJsonValue);
    }
    if (!isJsonObject(value)) {
        return false;
    }
    return Object.values(value).every(isJsonValue);
}

export function expectRecord(value: unknown, context: string): Record<string, unknown> {
    if (!isJsonObject(value)) {
        throw new TranslationError("invalid_json", `${context} must be an object`);
    }
    return value;
}

export function expectArray(value: unknown, context: string): unknown[] {
    if (!Array.isArray(value)) {
        throw new TranslationError("invalid_json", `${context} must be an array`);
    }
    return value;
}

export function expectString(value: unknown, context: string): string {
    if (typeof value !== "string") {
        throw new TranslationError("invalid_json", `${context} must be a string`);
    }
    return value;
}

export function expectNonEmptyString(value: unknown, context: string): string {
    const result = expectString(value, context);
    if (result.length === 0) {
        throw new TranslationError("invalid_json", `${context} must not be empty`);
    }
    return result;
}

// OpenAI-shaped upstreams legitimately serialize optional string fields as JSON `null`
// instead of omitting them — e.g. Chat Completions streams `delta.content: null` on
// tool-call deltas, `reasoning_content: null` on text deltas, and `"id": null` on
// tool-call argument chunks. For an optional field, `null` and absence carry the same
// meaning, so both read as `undefined`; required fields stay strict via `expectString`.
export function optionalString(value: unknown, context: string): string | undefined {
    return value === undefined || value === null ? undefined : expectString(value, context);
}

export function optionalFiniteNumber(value: unknown, context: string): number | undefined {
    if (value === undefined) {
        return undefined;
    }
    if (typeof value !== "number" || !Number.isFinite(value)) {
        throw new TranslationError("invalid_json", `${context} must be a finite number`);
    }
    return value;
}

export function optionalNonNegativeInteger(value: unknown, context: string): number | undefined {
    const result = optionalFiniteNumber(value, context);
    if (result !== undefined && (!Number.isInteger(result) || result < 0)) {
        throw new TranslationError("invalid_json", `${context} must be a non-negative integer`);
    }
    return result;
}

export function optionalBoolean(value: unknown, context: string): boolean | undefined {
    if (value === undefined) {
        return undefined;
    }
    if (typeof value !== "boolean") {
        throw new TranslationError("invalid_json", `${context} must be a boolean`);
    }
    return value;
}

export function expectJsonObject(value: unknown, context: string): JsonObject {
    if (!isJsonObject(value) || !isJsonValue(value)) {
        throw new TranslationError("invalid_json", `${context} must be a JSON object`);
    }
    return value;
}

export function parseJsonObjectText(value: string, context: string): JsonObject {
    let parsed: unknown;
    try {
        parsed = JSON.parse(value);
    } catch {
        throw new TranslationError("invalid_json", `${context} must contain a JSON object`);
    }
    return expectJsonObject(parsed, context);
}
