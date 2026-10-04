import { describe, expect, it } from "vitest";
import type {
    ApiKeyResponse,
    ModelResponse,
    ModelRouteResponse,
    ProviderResponse,
    UsageBucket,
    UsageBucketKey,
} from "/src/management-api/generated/v0";
import {
    daysNewestFirst,
    indexUsageEntities,
    usageApiKeyLabel,
    usageDimensionLabel,
    usageModelLabel,
    usageModelRouteLabel,
    usageProviderLabel,
} from "./usage-labels";

const providers: ReadonlyArray<ProviderResponse> = [
    {
        model_prefix: "",
        type: "api_key",
        provider_definition_name: "generic-openai-chat-completions",
        provider_definition_version: 1,
        format: "openai_chat_completion",
        id: 1,
        name: "local",
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

        is_enabled: true,
        discovery_state: "discovered",
        thinking_levels: null,
        max_input_tokens: null,
        max_output_tokens: null,
        thinking_levels_override: null,
        max_input_tokens_override: null,
        max_output_tokens_override: null,
        compaction_support: null,
        compaction_support_override: null,
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

const modelRoutes: ReadonlyArray<ModelRouteResponse> = [
    {
        is_automatic: false,
        id: 17,
        public_id: "coding",
        is_enabled: true,
        target: {
            type: "direct",
            model_id: 9,
        },
    },
];

const entities = {
    providers,
    models,
    apiKeys,
    modelRoutes,
};

const index = indexUsageEntities(entities);

describe("usage labels", () => {
    it("resolves a listed id through the entity lists", () => {
        expect(usageModelRouteLabel(index.modelRoutesById, 17)).toEqual({
            text: "coding",
            isListed: true,
        });
        expect(usageModelLabel(index.modelsById, 9)).toEqual({
            text: "qwen3:32b",
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
        expect(usageModelRouteLabel(index.modelRoutesById, 18)).toEqual({
            text: "Deleted route",
            isListed: false,
        });
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
            model_route_id: 17,
            provider_id: 1,
            api_key_id: 7,
            endpoint_type: "openai_responses",
            outcome: "client_aborted",
            is_stream: true,
            http_status: null,
        };

        expect(usageDimensionLabel("day", key, index)).toBe("2026-09-01");
        expect(usageDimensionLabel("model_route", key, index)).toBe("coding");
        expect(usageDimensionLabel("model", key, index)).toBe("qwen3:32b");
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

/** One counted day bucket on the given day, distinguishable by its request count. */
function dayBucket(day: string, requests: number): UsageBucket {
    return {
        key: {
            day,
        },
        requests,
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
    };
}

describe("daysNewestFirst", () => {
    it("orders the day series newest-first, keeping the answer's order within equal days", () => {
        const answer = [
            dayBucket("2026-09-01", 1),
            dayBucket("2026-09-03", 2),
            dayBucket("2026-09-02", 3),
            dayBucket("2026-09-03", 4),
        ];

        const displayed = daysNewestFirst(answer);

        // Newest day first; within the equal 09-03 days the answer's own
        // order stands — the comparison answers 0 for equal days and the
        // sort is stable.
        expect(displayed).toEqual([
            dayBucket("2026-09-03", 2),
            dayBucket("2026-09-03", 4),
            dayBucket("2026-09-02", 3),
            dayBucket("2026-09-01", 1),
        ]);
        // The answer is copied, never reordered in place.
        expect(answer.map((bucket) => bucket.requests)).toEqual([
            1,
            2,
            3,
            4,
        ]);
    });

    it("asserts the day part a day-series read guarantees", () => {
        const dayless: UsageBucket = {
            key: {},
            requests: 1,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        };

        // The comparator reads each compared bucket's day, so the assert
        // fires the moment two buckets are ordered against each other.
        expect(() =>
            daysNewestFirst([
                dayless,
                dayBucket("2026-09-01", 2),
            ]),
        ).toThrow(/the bucket key carries no day/);
    });
});
