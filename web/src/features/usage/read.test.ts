import { describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import { ManagementApiError } from "/src/management-api/errors";
import type {
    QueryUsageRequest,
    QueryUsageResponse,
    UsageBucket,
} from "/src/management-api/generated/v0";
import { readUsageBuckets } from "./read";

/** A `usage:query` stub answering the given buckets from `queryUsage`. */
function queryApi(buckets: ReadonlyArray<UsageBucket>): Pick<ManagementApi, "queryUsage"> {
    return {
        queryUsage: () =>
            Promise.resolve<QueryUsageResponse>({
                buckets: [
                    ...buckets,
                ],
            }),
    };
}

/** One bucket whose key is exactly the given partial key. */
function bucket(key: UsageBucket["key"]): UsageBucket {
    return {
        key,
        requests: 1,
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
    };
}

describe("readUsageBuckets", () => {
    it("answers the query untouched when every key carries the requested dimensions", async () => {
        const response = await readUsageBuckets(
            queryApi([
                bucket({
                    day: "2026-09-01",
                    provider_id: 1,
                }),
            ]),
            {
                group_by: [
                    "day",
                    "provider",
                ],
            } satisfies QueryUsageRequest,
        ).run(new AbortController().signal);

        expect(response.buckets).toHaveLength(1);
    });

    it("rejects a key missing a requested dimension as the usual contract failure", async () => {
        const failure = await readUsageBuckets(
            queryApi([
                bucket({
                    day: "2026-09-01",
                }),
            ]),
            {
                group_by: [
                    "day",
                    "provider",
                ],
            } satisfies QueryUsageRequest,
        )
            .run(new AbortController().signal)
            .catch((error: unknown) => error);

        expect(failure).toBeInstanceOf(ManagementApiError);
        expect((failure as ManagementApiError).category).toBe("contract");
        expect((failure as ManagementApiError).message).toContain(
            "the bucket key carries no provider_id",
        );
    });

    it("treats a `null` key part as present and constrains nothing the request did not ask", async () => {
        const nulledStatus = await readUsageBuckets(
            queryApi([
                bucket({
                    http_status: null,
                }),
            ]),
            {
                group_by: [
                    "http_status",
                ],
            } satisfies QueryUsageRequest,
        ).run(new AbortController().signal);
        expect(nulledStatus.buckets).toHaveLength(1);

        // A total query names no dimensions, so an empty key violates nothing.
        const total = await readUsageBuckets(
            queryApi([
                bucket({}),
            ]),
            {
                group_by: [],
            } satisfies QueryUsageRequest,
        ).run(new AbortController().signal);
        expect(total.buckets).toHaveLength(1);
    });
});
