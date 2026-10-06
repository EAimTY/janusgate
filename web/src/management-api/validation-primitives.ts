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

/**
 * One string the contract renders or stores verbatim: identity segments,
 * labels, and units are meaningful only when nonempty — an empty value is a
 * missing fact, so it is named instead of stored.
 */
export function expectNonEmptyString(value: unknown, context: string): string {
    const text = expectString(value, context);

    if (text === "") {
        throw new ContractViolationError(`${context} must not be empty`);
    }

    return text;
}

/**
 * Narrows one value to a closed string vocabulary: the member itself when
 * the value is one of `members`, else a contract violation naming the path,
 * the vocabulary's noun, and the offending value.
 */
export function expectOneOf<TValue extends string>(
    value: unknown,
    members: ReadonlyArray<TValue>,
    context: string,
    noun: string,
): TValue {
    const member = members.find((candidate) => candidate === value);

    if (member !== undefined) {
        return member;
    }

    throw new ContractViolationError(`${context} is not ${noun}: ${String(value)}`);
}

/** Whether an optional-and-nullable descriptor value carries content. */
export function isPresentValue(value: unknown): boolean {
    return value !== undefined && value !== null;
}

/**
 * Rejects record keys outside the set the walk consumed: a descriptor field
 * the bindings do not declare would be dropped by the rebuild below, and a
 * dropped field is a mistake to name inline — never one to send a truncated
 * payload about (the response parsers in `responses/` tolerate extras
 * instead, because those bodies are never rebuilt).
 */
export function expectDeclaredKeys(
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

/**
 * One closed record: the value narrowed to an object, then every present key
 * checked against `declared` — the record+declared-keys composition behind
 * every shape whose rebuild would silently drop an unknown field (the quota
 * contracts).
 */
export function expectDeclaredRecord(
    value: unknown,
    declared: ReadonlyArray<string>,
    context: string,
): object {
    const record = expectRecord(value, context);
    expectDeclaredKeys(record, declared, context);

    return record;
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

/**
 * One integer the wire guarantees JavaScript represents exactly: a SQL row
 * id, a priority, a microsecond timestamp. The optional `minimum` names the
 * smallest value the field may carry.
 */
export function expectSafeInteger(value: unknown, context: string, minimum?: number): number {
    const integer = expectInteger(value, context);

    if (!Number.isSafeInteger(integer) || (minimum !== undefined && integer < minimum)) {
        throw new ContractViolationError(
            minimum === undefined
                ? `${context} must be a safe integer`
                : `${context} must be a safe integer of at least ${minimum}`,
        );
    }

    return integer;
}

/**
 * One positive integer, the wire mirror of a `NonZero` count or duration: a
 * zero or negative budget, interval, or lifetime is a broken fact the authored
 * payload names inline instead of sending.
 */
export function expectPositiveInteger(value: unknown, context: string): number {
    const integer = expectInteger(value, context);

    if (integer <= 0) {
        throw new ContractViolationError(`${context} must be a positive integer`);
    }

    return integer;
}

/**
 * One never-recycled SQL row identifier: a safe integer of at least 1 — the
 * range every row id the gateway mints carries, whatever response embeds it.
 */
export function expectRowId(value: unknown, context: string): number {
    return expectSafeInteger(value, context, 1);
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

/**
 * One nullable integer the wire guarantees JavaScript represents exactly: the
 * nullable mirror-column timestamps and expiries, whose `null` is meaningful
 * and whose every carried value is exact.
 */
export function expectNullableSafeInteger(value: unknown, context: string): number | null {
    if (value === null) {
        return null;
    }

    if (typeof value !== "number" || !Number.isSafeInteger(value)) {
        throw new ContractViolationError(`${context} must be a safe integer or null`);
    }

    return value;
}
