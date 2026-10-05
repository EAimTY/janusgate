import { ContractViolationError } from "./errors";

/**
 * Scalar narrowing and list/envelope rebuilding shared by the management
 * response parsers and authored definition-section validators. Each failure
 * names the offending JSON path; concrete response parsers tolerate extra
 * fields while definitions validate declared keys before rebuilding.
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

/**
 * One list-shaped response: the record `response` holding the one array
 * member `member`, each entry narrowed through `parseElement`. Owns the
 * context-path grammar `response.<member>[i]` — a broken record or array
 * names the shape it broke, and a broken entry names the failing index.
 *
 * The mapped result keeps the returned object structurally identical to the
 * generated `List*Response` bindings.
 */
export function parseListResponse<const TMember extends string, TElement>(
    value: unknown,
    member: TMember,
    parseElement: (value: unknown, context: string) => TElement,
): { [TMemberName in TMember]: Array<TElement> } {
    const record = expectRecord(value, "response");
    const entries = expectArray(Reflect.get(record, member), `response.${member}`);

    // A generic computed key reads back as an index signature, so the literal
    // carries one assertion to stand as the mapped result; the key is `member`
    // itself, so the shape holds by construction on the same line.
    return {
        [member]: entries.map((entry, index) =>
            parseElement(entry, `response.${member}[${index}]`),
        ),
    } as { [TMemberName in TMember]: Array<TElement> };
}

/**
 * One envelope-shaped response: the record `response` holding the one member
 * `member`, narrowed through `parseElement`. Owns the context-path grammar
 * `response.<member>` — a broken record names the shape it broke, and a
 * broken member names the member directly.
 *
 * The mapped result keeps the returned object structurally identical to the
 * generated `*Envelope` bindings.
 */
export function parseEnvelopeResponse<const TMember extends string, TElement>(
    value: unknown,
    member: TMember,
    parseElement: (value: unknown, context: string) => TElement,
): { [TMemberName in TMember]: TElement } {
    const record = expectRecord(value, "response");

    // A generic computed key reads back as an index signature, so the literal
    // carries one assertion to stand as the mapped result; the key is `member`
    // itself, so the shape holds by construction on the same line.
    return {
        [member]: parseElement(Reflect.get(record, member), `response.${member}`),
    } as { [TMemberName in TMember]: TElement };
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

export function expectNullableString(value: unknown, context: string): string | null {
    if (value === null) {
        return null;
    }

    if (typeof value !== "string") {
        throw new ContractViolationError(`${context} must be a string or null`);
    }

    return value;
}

export function expectNullableInteger(value: unknown, context: string): number | null {
    if (value === null) {
        return null;
    }

    if (typeof value !== "number" || !Number.isInteger(value)) {
        throw new ContractViolationError(`${context} must be an integer or null`);
    }

    return value;
}
