import { describe, expect, test } from "vitest";
import { ManagementApiError } from "/src/management-api/errors";
import type { QueryUsageRequest, QueryUsageResponse } from "/src/management-api/generated/v0";
import { readUsageBuckets, usageListRequest } from "./read";

/**
 * The usage reads the screens own: the listing request carrying the page
 * size, the cursor only when the stream continues, and the filter only when
 * something is chosen; and the aggregates read proving its answer inside the
 * cycle — a bucket key missing a requested dimension is a contract failure
 * while the cycle can still state its diagnosis, never at render time.
 */

describe("usageListRequest", () => {
    test("carries the page size and omits the absent cursor and the empty filter", () => {
        expect(usageListRequest({}, null)).toEqual({
            limit: 100,
        });
    });

    test("carries the cursor when the stream continues", () => {
        expect(usageListRequest({}, 1_700_000_000_000_000)).toEqual({
            limit: 100,
            before: 1_700_000_000_000_000,
        });
    });

    test("carries the filter exactly when something is chosen", () => {
        expect(
            usageListRequest(
                {
                    outcome: [
                        "upstream_error",
                    ],
                },
                null,
            ),
        ).toEqual({
            limit: 100,
            filter: {
                outcome: [
                    "upstream_error",
                ],
            },
        });
    });
});

describe("readUsageBuckets", () => {
    const REQUEST: QueryUsageRequest = {
        group_by: [
            "day",
            "outcome",
        ],
    };

    function apiAnswering(buckets: QueryUsageResponse["buckets"]): {
        queryUsage: (
            request: QueryUsageRequest,
            signal?: AbortSignal,
        ) => Promise<QueryUsageResponse>;
    } {
        return {
            queryUsage: async () => ({
                buckets,
            }),
        };
    }

    test("answers the query when every requested dimension is present", async () => {
        const read = readUsageBuckets(
            apiAnswering([
                {
                    key: {
                        day: "2026-10-10",
                        outcome: "completed",
                    },
                    requests: 1,
                    input_tokens: 0,
                    output_tokens: 0,
                    cached_tokens: 0,
                    cache_write_tokens: 0,
                    reasoning_tokens: 0,
                },
            ]),
            REQUEST,
        );

        const result = await read.run(new AbortController().signal);
        expect(result.buckets).toHaveLength(1);
    });

    test("treats a key part of null as present — the NULL status bucket", async () => {
        const read = readUsageBuckets(
            apiAnswering([
                {
                    key: {
                        day: "2026-10-10",
                        http_status: null,
                    },
                    requests: 1,
                    input_tokens: 0,
                    output_tokens: 0,
                    cached_tokens: 0,
                    cache_write_tokens: 0,
                    reasoning_tokens: 0,
                },
            ]),
            {
                group_by: [
                    "day",
                    "http_status",
                ],
            },
        );

        await expect(read.run(new AbortController().signal)).resolves.toBeTruthy();
    });

    test("fails the cycle with a contract diagnosis when a requested dimension is missing", async () => {
        const read = readUsageBuckets(
            apiAnswering([
                {
                    key: {
                        day: "2026-10-10",
                    },
                    requests: 1,
                    input_tokens: 0,
                    output_tokens: 0,
                    cached_tokens: 0,
                    cache_write_tokens: 0,
                    reasoning_tokens: 0,
                },
            ]),
            REQUEST,
        );

        const failure = await read.run(new AbortController().signal).then(
            () => null,
            (error: unknown) => error,
        );

        expect(failure).toBeInstanceOf(ManagementApiError);
        expect((failure as ManagementApiError).category).toBe("contract");
        expect((failure as ManagementApiError).message).toContain(
            "the bucket key carries no outcome",
        );
    });
});
