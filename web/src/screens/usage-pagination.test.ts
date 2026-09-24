import { describe, expect, it } from "vitest";
import type { ApiLogResponse } from "/src/management-api/generated/v0";
import { nextUsagePage, USAGE_PAGE_SIZE, usageCursor } from "./usage-pagination";

/** The base instant every fixture row's stamp offsets, marking rows by it. */
const CURSOR_BASE = 1_700_000_000_000_000;

function row(offset: number): ApiLogResponse {
    return {
        started_at: CURSOR_BASE + offset,
        api_key_id: 7,
        provider_id: 1,
        model_id: 9,
        endpoint_type: "openai_chat_completions",
        is_stream: false,
        http_status: 200,
        outcome: "completed",
        duration: 120,
        input_tokens: 10,
        output_tokens: 20,
        cached_tokens: null,
        cache_write_tokens: null,
        reasoning_tokens: null,
    };
}

function rowsFromIds(ids: ReadonlyArray<number>): ReadonlyArray<ApiLogResponse> {
    return ids.map(row);
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
        ).toBe(CURSOR_BASE + 2);
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
            CURSOR_BASE + 5,
            CURSOR_BASE + 4,
            CURSOR_BASE + 3,
            CURSOR_BASE + 2,
        ]);
    });
});
