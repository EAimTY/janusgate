import { describe, expect, test } from "vitest";
import type { ApiLogResponse } from "/src/management-api/generated/v0";
import { nextUsagePage, USAGE_PAGE_SIZE, usageCursor } from "./pagination";

/**
 * The usage stream's pagination: the `before` cursor being the last
 * returned row's `started_at` (strictly older semantics, so the cursor row
 * never reappears), and the stream continuing only when the fetched page
 * filled its full size.
 */

function log(startedAt: number): ApiLogResponse {
    return {
        started_at: startedAt,
        api_key_id: 1,
        provider_id: 1,
        model_id: 1,
        model_route_id: 1,
        upstream_http_status: null,
        endpoint_type: "openai_chat_completions",
        is_stream: false,
        http_status: 200,
        outcome: "completed",
        duration: 10,
        input_tokens: null,
        output_tokens: null,
        cached_tokens: null,
        cache_write_tokens: null,
        reasoning_tokens: null,
    };
}

describe("usageCursor", () => {
    test("answers the last row's started_at, the strictly-older bound", () => {
        expect(
            usageCursor([
                log(300),
                log(200),
                log(100),
            ]),
        ).toBe(100);
    });

    test("answers null for an empty listing — the first page starts at the newest row", () => {
        expect(usageCursor([])).toBeNull();
    });
});

describe("nextUsagePage", () => {
    test("appends the fetched page after the current rows in order", () => {
        const state = nextUsagePage(
            [
                log(300),
                log(200),
            ],
            [
                log(100),
                log(50),
            ],
        );

        expect(state.rows.map((row) => row.started_at)).toEqual([
            300,
            200,
            100,
            50,
        ]);
    });

    test("continues only when the fetched page filled its full size", () => {
        expect(
            nextUsagePage(
                [],
                Array.from(
                    {
                        length: USAGE_PAGE_SIZE,
                    },
                    (_, index) => log(index),
                ),
            ).hasMore,
        ).toBe(true);
        expect(
            nextUsagePage(
                [],
                [
                    log(1),
                ],
            ).hasMore,
        ).toBe(false);
        expect(nextUsagePage([], []).hasMore).toBe(false);
    });
});
