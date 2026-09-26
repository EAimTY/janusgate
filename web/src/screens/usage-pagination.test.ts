import { describe, expect, it } from "vitest";
import type { ApiLogResponse } from "/src/management-api/generated/v0";
import { API_LOG_CURSOR_BASE, apiLogRow } from "./screen-test-support";
import { nextUsagePage, USAGE_PAGE_SIZE, usageCursor } from "./usage-pagination";

function rowsFromIds(ids: ReadonlyArray<number>): ReadonlyArray<ApiLogResponse> {
    return ids.map((offset) => apiLogRow(offset));
}

/** A page filled to `USAGE_PAGE_SIZE`, ids descending as the gateway orders them. */
function fullPageRows(): ReadonlyArray<ApiLogResponse> {
    return rowsFromIds(
        Array.from(
            {
                length: USAGE_PAGE_SIZE,
            },
            (_, index) => USAGE_PAGE_SIZE - index,
        ),
    );
}

describe("usage pagination", () => {
    it("continues after a page filled to its full size", () => {
        const fullPage = fullPageRows();
        const continuation = nextUsagePage([], fullPage);

        expect(continuation.hasMore).toBe(true);
        expect(continuation.rows).toEqual(fullPage);
    });

    it("ends the stream on a page shorter than the full size", () => {
        const continuation = nextUsagePage(
            rowsFromIds([
                3,
                2,
            ]),
            rowsFromIds([
                1,
            ]),
        );

        expect(continuation.hasMore).toBe(false);
    });

    it("appends nothing on an empty page and ends the stream", () => {
        const current = rowsFromIds([
            2,
            1,
        ]);
        const continuation = nextUsagePage(current, []);

        expect(continuation.rows).toEqual(current);
        expect(continuation.hasMore).toBe(false);
    });

    it("takes the next request's cursor from the last returned row's stamp", () => {
        expect(
            usageCursor(
                rowsFromIds([
                    8,
                    5,
                    2,
                ]),
            ),
        ).toBe(API_LOG_CURSOR_BASE + 2);
        expect(usageCursor([])).toBe(null);
    });

    it("appends a fetched page after the current rows, preserving order", () => {
        const continuation = nextUsagePage(
            rowsFromIds([
                5,
                4,
            ]),
            rowsFromIds([
                3,
                2,
            ]),
        );

        expect(continuation.rows.map((listed) => listed.started_at)).toEqual([
            API_LOG_CURSOR_BASE + 5,
            API_LOG_CURSOR_BASE + 4,
            API_LOG_CURSOR_BASE + 3,
            API_LOG_CURSOR_BASE + 2,
        ]);
    });
});
