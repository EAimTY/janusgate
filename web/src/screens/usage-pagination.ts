import type { ApiLogResponse } from "/src/management-api/generated/v0";

/** One `usage:list` page: the gateway's default of 100 rows. */
export const USAGE_PAGE_SIZE = 100;

/** The stream state one fetched page produces: the rows to hold and whether the stream continues past them. */
type UsageContinuation = {
    readonly rows: ReadonlyArray<ApiLogResponse>;
    /** True only when the fetched page filled its full size — a shorter page proves nothing older is recorded. */
    readonly hasMore: boolean;
};

/**
 * The `before_id` cursor the next `usage:list` request carries: the last returned row's
 * id, whose semantics are strictly older (`id < before_id`), so the cursor row itself
 * never reappears. `null` when nothing is listed — the first page starts at the latest
 * row instead.
 */
export function usageCursor(rows: ReadonlyArray<ApiLogResponse>): number | null {
    const last = rows[rows.length - 1];

    return last === undefined ? null : last.id;
}

/**
 * The stream state after one fetched page lands: the page appended after the current
 * rows in order, continuing only when it filled its full size. A failed page never
 * reaches here — the caller keeps the current state untouched for it.
 */
export function nextUsagePage(
    currentRows: ReadonlyArray<ApiLogResponse>,
    fetchedPage: ReadonlyArray<ApiLogResponse>,
): UsageContinuation {
    return {
        rows: [
            ...currentRows,
            ...fetchedPage,
        ],
        hasMore: fetchedPage.length === USAGE_PAGE_SIZE,
    };
}
