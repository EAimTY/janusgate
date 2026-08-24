import { CANONICAL_THINKING_LEVELS, type ThinkingLevel } from "/config";
import { TranslationError } from "./errors";

export const THINKING_LEVELS = CANONICAL_THINKING_LEVELS;
export type { ThinkingLevel };

export function parseThinkingLevel(
    value: unknown,
    parameterName: string,
): ThinkingLevel | undefined {
    if (value === undefined) {
        return undefined;
    }
    if (typeof value !== "string" || !isThinkingLevel(value)) {
        throw new TranslationError(
            "unsupported_parameter",
            `${parameterName} must be one of ${THINKING_LEVELS.join(", ")}`,
        );
    }
    return value;
}

export function isThinkingLevel(value: string): value is ThinkingLevel {
    return THINKING_LEVELS.some((level) => level === value);
}

export function clampThinkingLevel(
    requested: ThinkingLevel | undefined,
    supported: readonly ThinkingLevel[],
): ThinkingLevel | undefined {
    if (requested === undefined) {
        return undefined;
    }
    if (!isThinkingLevel(requested)) {
        throw new TranslationError("unsupported_parameter", "reasoning effort is not canonical");
    }

    const requestedIndex = THINKING_LEVELS.indexOf(requested);
    for (let index = requestedIndex; index >= 0; index -= 1) {
        const candidate = THINKING_LEVELS[index];
        if (candidate !== undefined && supported.includes(candidate)) {
            return candidate;
        }
    }
    return undefined;
}
