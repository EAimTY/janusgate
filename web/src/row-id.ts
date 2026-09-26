/**
 * The row-identifier route-reading rule, stated once for every place a route
 * carries a row: a path segment, a models filter, a usage filter, and a usage
 * query's subject alike. A row identifier is a plain decimal integer within
 * the safe-integer range; anything else reads as absent.
 */

/**
 * Whether a route value's text carries a row identifier's shape at all: a
 * plain decimal integer, unsigned and unbounded. The safe-integer bound is
 * applied by the readers on top of it.
 */
function isRowIdText(value: string): boolean {
    return /^\d+$/.test(value);
}

/**
 * Reads a row identifier out of a path segment or query value. Only a plain
 * decimal integer addresses a row; anything else reads as absent.
 */
export function parseRowIdentifier(value: string | null): number | null {
    if (value === null || !isRowIdText(value)) {
        return null;
    }

    const identifier = Number(value);
    return Number.isSafeInteger(identifier) ? identifier : null;
}
