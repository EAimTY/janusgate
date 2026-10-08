import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";

/**
 * Per-field equality overrides for `isSameRowFields`: a field whose values
 * need content equality beyond the default — a structured policy object
 * compared by its canonical rules, say — names its own comparator. Fields
 * without an override compare by the walk's default.
 */
type RowFieldEquality<TRow extends object> = Partial<{
    [TFieldKey in keyof TRow]: (previous: TRow[TFieldKey], fresh: TRow[TFieldKey]) => boolean;
}>;

/**
 * Whether two rows of one generated wire shape are field-for-field equal. The
 * comparison walks every key the previous row carries and requires the fresh
 * row to carry exactly those keys with equal values, so a tagged union's
 * members — whose key sets differ — never answer equal, and a row that grows
 * another generated column joins the walk automatically instead of silently
 * comparing unequal rows as equal. Most fields are primitives compared with
 * `===`; array values are compared element-wise, since equal content in
 * distinct instances must keep the row's identity. A field named in
 * `isSameField` is compared by its own comparator instead, so a structured
 * field can answer content equality; every other field keeps the default.
 * Were a row ever to grow another structured field with no override, equal
 * content in distinct instances would compare unequal, degrading benignly to
 * adopting the fresh row — never to keeping a stale one.
 */
export function isSameRowFields<TRow extends object>(
    previous: TRow,
    fresh: TRow,
    isSameField: RowFieldEquality<TRow> = {},
): boolean {
    const previousKeys = Object.keys(previous) as (keyof TRow)[];
    const freshKeys = Object.keys(fresh) as (keyof TRow)[];
    return (
        previousKeys.length === freshKeys.length &&
        previousKeys.every((key) => isSameFieldValue(previous, fresh, key, isSameField))
    );
}

/** One field's equality under the walk: its override where named, the default otherwise. */
function isSameFieldValue<TRow extends object, TFieldKey extends keyof TRow>(
    previous: TRow,
    fresh: TRow,
    fieldKey: TFieldKey,
    isSameField: RowFieldEquality<TRow>,
): boolean {
    const isSameOverride = isSameField[fieldKey];
    if (isSameOverride !== undefined) {
        return isSameOverride(previous[fieldKey], fresh[fieldKey]);
    }

    const before: unknown = previous[fieldKey];
    const after: unknown = fresh[fieldKey];

    if (Array.isArray(before) || Array.isArray(after)) {
        return (
            Array.isArray(before) &&
            Array.isArray(after) &&
            before.length === after.length &&
            before.every((entry, index) => entry === after[index])
        );
    }

    return before === after;
}

/**
 * Whether two nullable record maps carry the same entries: `null` meets only
 * `null`, and two records meet when they name the same keys with equal
 * values — key order never carries facts. The response parsers rebuild these
 * maps as fresh objects on every read, so instance identity can never hold
 * and content is the only equality that keeps a row's identity.
 */
function isSameNullableRecord<TValue extends string>(
    previous: {
        [key: string]: TValue;
    } | null,
    fresh: {
        [key: string]: TValue;
    } | null,
): boolean {
    if (previous === null || fresh === null) {
        return previous === fresh;
    }

    const previousKeys = Object.keys(previous);

    return (
        previousKeys.length === Object.keys(fresh).length &&
        previousKeys.every((key) => previous[key] === fresh[key])
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

/**
 * Whether a fresh read answers the same model row, content and all. The two
 * compaction-facts columns are plain maps the response parser rebuilds fresh
 * per read, so they compare by their entries — the walk's reference default
 * would read every content-equal re-read as changed and remount the row body
 * on each refresh.
 */
export function isSameModelRow(previous: ModelResponse, fresh: ModelResponse): boolean {
    return isSameRowFields(previous, fresh, {
        compaction_support: isSameNullableRecord,
        compaction_support_override: isSameNullableRecord,
    });
}
