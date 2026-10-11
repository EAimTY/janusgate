import { describe, expect, test } from "vitest";
import { parseListUsageResponse, parseQueryUsageResponse } from "./usage";

/**
 * The usage response parsers: the accounting row carried verbatim — stamps
 * and numeric facts only, no name riding along — and the aggregation bucket
 * whose composite key narrows each present dimension through its correlated
 * vocabulary while an absent dimension passes through, exactly as the wire
 * omits it.
 */

/** One complete accounting row fixture. */
const LOG_ROW = {
    started_at: 1_700_000_000_000_000,
    api_key_id: 3,
    provider_id: 5,
    model_id: 7,
    model_route_id: 11,
    upstream_http_status: null,
    endpoint_type: "openai_chat_completions",
    is_stream: true,
    http_status: 200,
    outcome: "completed",
    duration: 812,
    input_tokens: 120,
    output_tokens: 45,
    cached_tokens: null,
    cache_write_tokens: null,
    reasoning_tokens: 10,
};

/** One bucket keyed by day and outcome. */
const DAY_BUCKET = {
    key: {
        day: "2026-10-09",
        outcome: "upstream_error",
    },
    requests: 4,
    input_tokens: 500,
    output_tokens: 100,
    cached_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
};

describe("usage listing parser", () => {
    test("answers the accounting columns verbatim, nullable mirrors included", () => {
        const listing = parseListUsageResponse({
            api_logs: [
                LOG_ROW,
            ],
        });

        const row = listing.api_logs[0];
        if (row === undefined) throw new Error("the fixture listed no row");
        expect(row.started_at).toBe(1_700_000_000_000_000);
        expect(row.upstream_http_status).toBeNull();
        expect(row.cached_tokens).toBeNull();
        expect(row.outcome).toBe("completed");
    });

    test("rejects a foreign endpoint type and outcome", () => {
        expect(() =>
            parseListUsageResponse({
                api_logs: [
                    {
                        ...LOG_ROW,
                        endpoint_type: "graphql",
                    },
                ],
            }),
        ).toThrowError("is not an endpoint type: graphql");
        expect(() =>
            parseListUsageResponse({
                api_logs: [
                    {
                        ...LOG_ROW,
                        outcome: "timeout",
                    },
                ],
            }),
        ).toThrowError("is not a request outcome: timeout");
    });
});

describe("usage aggregates parser", () => {
    test("answers buckets whose keys narrow per present dimension", () => {
        const query = parseQueryUsageResponse({
            buckets: [
                DAY_BUCKET,
            ],
        });

        const bucket = query.buckets[0];
        if (bucket === undefined) throw new Error("the fixture listed no bucket");
        expect(bucket.key.day).toBe("2026-10-09");
        expect(bucket.key.outcome).toBe("upstream_error");
        expect(bucket.key).not.toHaveProperty("model_id");
    });

    test("carries the NULL status bucket's null as present, never dropped", () => {
        const query = parseQueryUsageResponse({
            buckets: [
                {
                    ...DAY_BUCKET,
                    key: {
                        http_status: null,
                    },
                },
            ],
        });

        const bucket = query.buckets[0];
        if (bucket === undefined) throw new Error("the fixture listed no bucket");
        expect(bucket.key.http_status).toBeNull();
    });

    test("rejects a wrongly-typed present dimension", () => {
        expect(() =>
            parseQueryUsageResponse({
                buckets: [
                    {
                        ...DAY_BUCKET,
                        key: {
                            is_stream: "yes",
                        },
                    },
                ],
            }),
        ).toThrowError("response.buckets[0].key.is_stream must be a boolean");
    });

    test("rejects a bucket whose numeric sums are not integers", () => {
        expect(() =>
            parseQueryUsageResponse({
                buckets: [
                    {
                        ...DAY_BUCKET,
                        requests: 1.5,
                    },
                ],
            }),
        ).toThrowError("response.buckets[0].requests must be an integer");
    });
});
