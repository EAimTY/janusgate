import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";

/**
 * Whether two rows of one generated wire shape are field-for-field equal. The
 * comparison walks every key the previous row carries and requires the fresh
 * row to carry exactly those keys with equal values, so a tagged union's
 * members — whose key sets differ — never answer equal. Most fields are
 * primitives compared with `===`; the model row's capability level sets are
 * arrays compared element-wise, since equal content in distinct instances
 * must keep the row's identity. Were a row ever to grow another structured
 * field, equal content in distinct instances would compare unequal,
 * degrading benignly to adopting the fresh row — never to keeping a stale one.
 */
function isSameRowFields<TRow extends object>(previous: TRow, fresh: TRow): boolean {
    const previousKeys = Object.keys(previous) as (keyof TRow)[];
    const freshKeys = Object.keys(fresh) as (keyof TRow)[];
    return (
        previousKeys.length === freshKeys.length &&
        previousKeys.every((key) => {
            const before: unknown = previous[key];
            const after: unknown = fresh[key];

            if (Array.isArray(before) || Array.isArray(after)) {
                return (
                    Array.isArray(before) &&
                    Array.isArray(after) &&
                    before.length === after.length &&
                    before.every((entry, index) => entry === after[index])
                );
            }

            return before === after;
        })
    );
}

/**
 * Whether a fresh read answers the same provider row the screen already
 * holds, content and all. An equal row keeps its identity, so a refresh that
 * changes nothing never remounts — and never resets — a form opened on the
 * row (README §6.3's one-pattern reads must not cost an operator input).
 */
export function isSameProviderRow(previous: ProviderResponse, fresh: ProviderResponse): boolean {
    return isSameRowFields(previous, fresh);
}

/** Whether a fresh read answers the same model row, content and all. */
export function isSameModelRow(previous: ModelResponse, fresh: ModelResponse): boolean {
    return isSameRowFields(previous, fresh);
}
