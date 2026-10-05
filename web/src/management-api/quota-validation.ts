import { ContractViolationError } from "./errors";
import { expectArray, expectRecord, expectString } from "./validation-primitives";

/** Rejects a contract violation at a safe, known JSON path. */
export function invalid(context: string): never {
    throw new ContractViolationError(`${context} violates the quota contract`);
}

/** Closed quota shape; unknown fields cannot be discarded during editing. */
export function shape(value: unknown, keys: readonly string[], context: string): object {
    const record = expectRecord(value, context);
    for (const key of Object.keys(record)) {
        if (!keys.includes(key)) invalid(`${context}.${key}`);
    }
    return record;
}

/** Narrows a generated closed vocabulary without unchecked assertions. */
export function choice<T extends string>(
    value: unknown,
    choices: readonly T[],
    context: string,
): T {
    const selected = choices.find((candidate) => candidate === value);
    return selected ?? invalid(context);
}

/** Nonempty text is retained verbatim, including meaningful spaces. */
export function text(value: unknown, context: string): string {
    const result = expectString(value, context);
    return result.length > 0 ? result : invalid(context);
}

/** Exact authored model token; selectors never derive IDs from a presentation label. */
export function modelIdentifier(value: unknown, context: string): string {
    const identifier = text(value, context);
    if (!/^[!-~]+$/.test(identifier) || /[/:]/.test(identifier)) invalid(context);
    return identifier;
}

/** A frontend-safe integer, optionally positive. */
export function integer(value: unknown, context: string, isPositive = false): number {
    if (typeof value !== "number" || !Number.isSafeInteger(value) || (isPositive && value <= 0)) {
        invalid(context);
    }
    return value;
}

/** Finite informational amounts keep decimals, over-limit use, and normalize negative zero. */
export function amount(value: unknown, context: string, isPositive = false): number {
    if (
        typeof value !== "number" ||
        !Number.isFinite(value) ||
        value < 0 ||
        (isPositive && value === 0) ||
        (Number.isInteger(value) && !Number.isSafeInteger(value))
    ) {
        invalid(context);
    }
    return value === 0 ? 0 : value;
}

/** Validates an exact real calendar date without constructing a timezone-dependent instant. */
export function isQuotaDate(value: string): boolean {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return false;
    const year = Number(value.slice(0, 4));
    const month = Number(value.slice(5, 7));
    const day = Number(value.slice(8, 10));
    const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
    const days = [
        31,
        leap ? 29 : 28,
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    const maximum = days[month - 1];
    return maximum !== undefined && day >= 1 && day <= maximum;
}

/** Object-member paths share the existing descriptor language. */
export function parseQuotaPath(value: unknown, context: string): string[] {
    return expectArray(value, context).map((part, index) => text(part, `${context}[${index}]`));
}
