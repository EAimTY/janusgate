import { describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import { ManagementApiError } from "/src/management-api/errors";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
    QueryUsageRequest,
    QueryUsageResponse,
    UsageBucket,
    UsageBucketKey,
} from "/src/management-api/generated/v0";
import {
    indexUsageEntities,
    readUsageBuckets,
    usageApiKeyLabel,
    usageDimensionLabel,
    usageModelLabel,
    usageProviderLabel,
} from "./usage-labels";

const providers: ReadonlyArray<ProviderResponse> = [
    {
        type: "openai_chat_completion",
        id: 1,
        name: "local",
        model_prefix: "local-",
        is_enabled: true,
        base_url: "https://upstream.example.com/v1",
    },
];

const models: ReadonlyArray<ModelResponse> = [
    {
        id: 9,
        provider_id: 1,
        provider: "local",
        upstream_id: "qwen3:32b",
        public_id: "local-qwen3-32b",
        id_override: null,
        is_enabled: true,
        discovery_state: "discovered",
    },
];

const apiKeys: ReadonlyArray<ApiKeyResponse> = [
    {
        id: 7,
        name: "service-a",
        key: "sk-live",
        is_enabled: true,
        created_at: 1_700_000_000_000_000,
    },
];

const entities = {
    providers,
    models,
    apiKeys,
};

const index = indexUsageEntities(entities);

describe("usage labels", () => {
    it("resolves a listed id through the entity lists", () => {
        expect(usageModelLabel(index.modelsById, 9)).toEqual({
            text: "local-qwen3-32b",
            isListed: true,
        });
        expect(usageProviderLabel(index.providersById, 1)).toEqual({
            text: "local",
            isListed: true,
        });
        expect(usageApiKeyLabel(index.apiKeysById, 7)).toEqual({
            text: "service-a",
            isListed: true,
        });
    });

    it("names a deleted row by its kind, never by a guessed name or number", () => {
        expect(usageModelLabel(index.modelsById, 11)).toEqual({
            text: "Deleted model",
            isListed: false,
        });
        expect(usageProviderLabel(index.providersById, 3)).toEqual({
            text: "Deleted provider",
            isListed: false,
        });
        expect(usageApiKeyLabel(index.apiKeysById, 8)).toEqual({
            text: "Deleted key",
            isListed: false,
        });
    });

    it("renders each requested bucket-key dimension in the operator's words", () => {
        const key: UsageBucketKey = {
            day: "2026-09-01",
            model_id: 9,
            provider_id: 1,
            api_key_id: 7,
            endpoint_type: "openai_responses",
            outcome: "client_aborted",
            is_stream: true,
            http_status: null,
        };

        expect(usageDimensionLabel("day", key, index)).toBe("2026-09-01");
        expect(usageDimensionLabel("model", key, index)).toBe("local-qwen3-32b");
        expect(usageDimensionLabel("provider", key, index)).toBe("local");
        expect(usageDimensionLabel("api_key", key, index)).toBe("service-a");
        expect(usageDimensionLabel("endpoint_type", key, index)).toBe("Responses");
        expect(usageDimensionLabel("outcome", key, index)).toBe("Client aborted");
        expect(usageDimensionLabel("is_stream", key, index)).toBe("Streamed");
        // The `NULL` status bucket is a real bucket: an em dash, never a crash.
        expect(usageDimensionLabel("http_status", key, index)).toBe("—");
        expect(
            usageDimensionLabel(
                "http_status",
                {
                    ...key,
                    http_status: 429,
                },
                index,
            ),
        ).toBe("429");
        expect(
            usageDimensionLabel(
                "is_stream",
                {
                    ...key,
                    is_stream: false,
                },
                index,
            ),
        ).toBe("Non-streamed");
    });

    it("treats a key field missing for a requested dimension as a contract violation", () => {
        expect(() => usageDimensionLabel("day", {}, index)).toThrow(
            /the bucket key carries no day/,
        );
        expect(() =>
            usageDimensionLabel(
                "model",
                {
                    day: "2026-09-01",
                },
                index,
            ),
        ).toThrow(/the bucket key carries no model_id/);
    });
});

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
