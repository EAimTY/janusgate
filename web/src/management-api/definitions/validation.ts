import { ContractViolationError } from "../errors";

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

/** Whether an optional-and-nullable descriptor value carries content. */
export function isPresentValue(value: unknown): boolean {
    return value !== undefined && value !== null;
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
