/**
 * Maps listing rows by their id, ready for O(1) lookups. Row ids are unique
 * by contract, so each id answers exactly one row.
 */
export function indexRowsById<
    TRow extends {
        readonly id: number;
    },
>(rows: ReadonlyArray<TRow>): ReadonlyMap<number, TRow> {
    return new Map(
        rows.map((row) => [
            row.id,
            row,
        ]),
    );
}
