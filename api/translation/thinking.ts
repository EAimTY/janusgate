import { TranslationError } from "./errors";

/**
 * Reasoning effort is an opaque non-empty string, forwarded to upstreams verbatim whenever
 * their wire format can express it. Only shape is validated here — never a value set.
 */
export function parseReasoningEffort(value: unknown, parameterName: string): string | undefined {
    if (value === undefined) {
        return undefined;
    }
    if (typeof value !== "string" || value.length === 0) {
        throw new TranslationError(
            "unsupported_parameter",
            `${parameterName} must be a non-empty string`,
        );
    }
    return value;
}
