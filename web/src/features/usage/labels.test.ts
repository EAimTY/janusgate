import { describe, expect, test } from "vitest";
import type { UsageBucket, UsageBucketKey } from "/src/management-api/generated/v0";
import {
    daysNewestFirst,
    indexUsageEntities,
    usageApiKeyLabel,
    usageDimensionLabel,
    usageModelLabel,
    usageModelRouteLabel,
    usageProviderLabel,
} from "./labels";
import { EMPTY_USAGE_ENTITIES, type UsageEntities } from "./read";

/**
 * The usage displays' labels: entity ids resolved through the id index with
 * deleted ids staying meaningful attribution, the day series newest-first
 * with the answer's own order kept inside a day, and each dimension's key
 * part in the operator's words — the NULL status bucket reading as an em
 * dash, never as a missing one.
 */

const ENTITIES: UsageEntities = {
    providers: [
        {
            type: "api_key",
            id: 1,
            name: "acme",
            model_prefix: "acme",
            is_enabled: true,
            provider_definition_name: "d",
            provider_definition_version: 1,
            format: "openai_chat_completion",
            base_url: null,
        },
    ],
    models: [
        {
            id: 10,
            provider_id: 1,
            provider: "acme",
            upstream_id: "gpt-x",
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
    ],
    apiKeys: [
        {
            id: 3,
            name: "batch",
            key: "jkc-1",
            is_enabled: true,
            created_at: 0,
            access_control: {
                default_access: "deny",
                owner_defaults: [],
                route_overrides: [],
            },
        },
    ],
    modelRoutes: [
        {
            id: 100,
            public_id: "gpt-x",
            is_automatic: false,
            is_enabled: true,
            target: {
                type: "direct",
                model_id: 10,
            },
        },
    ],
};

function bucket(key: UsageBucketKey, requests = 1): UsageBucket {
    return {
        key,
        requests,
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
    };
}

describe("entity labels", () => {
    test("answer the current name for a listed row", () => {
        const index = indexUsageEntities(ENTITIES);

        expect(usageProviderLabel(index.providersById, 1)).toEqual({
            text: "acme",
            isListed: true,
        });
        expect(usageModelLabel(index.modelsById, 10)).toEqual({
            text: "gpt-x",
            isListed: true,
        });
        expect(usageApiKeyLabel(index.apiKeysById, 3)).toEqual({
            text: "batch",
            isListed: true,
        });
        expect(usageModelRouteLabel(index.modelRoutesById, 100)).toEqual({
            text: "gpt-x",
            isListed: true,
        });
    });

    test("answer what kind of row is gone when the id is no longer listed", () => {
        const index = indexUsageEntities(ENTITIES);

        expect(usageProviderLabel(index.providersById, 999).isListed).toBe(false);
        expect(usageModelLabel(index.modelsById, 999).isListed).toBe(false);
        expect(usageApiKeyLabel(index.apiKeysById, 999).isListed).toBe(false);
        expect(usageModelRouteLabel(index.modelRoutesById, 999).isListed).toBe(false);
    });

    test("index nothing without failing on the empty lists", () => {
        const index = indexUsageEntities(EMPTY_USAGE_ENTITIES);

        expect(index.providersById.size).toBe(0);
        expect(usageProviderLabel(index.providersById, 1).isListed).toBe(false);
    });
});

describe("daysNewestFirst", () => {
    test("orders the day series newest first", () => {
        const ordered = daysNewestFirst([
            bucket({
                day: "2026-10-08",
            }),
            bucket({
                day: "2026-10-10",
            }),
            bucket({
                day: "2026-10-09",
            }),
        ]);

        expect(ordered.map((entry) => entry.key.day)).toEqual([
            "2026-10-10",
            "2026-10-09",
            "2026-10-08",
        ]);
    });

    test("keeps the answer's own order inside equal days", () => {
        const first = bucket(
            {
                day: "2026-10-10",
                outcome: "completed",
            },
            1,
        );
        const second = bucket(
            {
                day: "2026-10-10",
                outcome: "internal",
            },
            2,
        );

        const ordered = daysNewestFirst([
            first,
            second,
        ]);

        expect(ordered[0]).toBe(first);
        expect(ordered[1]).toBe(second);
    });
});

describe("usageDimensionLabel", () => {
    const index = indexUsageEntities(ENTITIES);

    test("answers the day verbatim", () => {
        expect(
            usageDimensionLabel(
                "day",
                {
                    day: "2026-10-10",
                },
                index,
            ),
        ).toBe("2026-10-10");
    });

    test("answers entity dimensions through the index, deleted ids staying meaningful", () => {
        expect(
            usageDimensionLabel(
                "provider",
                {
                    provider_id: 1,
                },
                index,
            ),
        ).toBe("acme");
        expect(
            usageDimensionLabel(
                "model",
                {
                    model_id: 10,
                },
                index,
            ),
        ).toBe("gpt-x");
        expect(
            usageDimensionLabel(
                "api_key",
                {
                    api_key_id: 3,
                },
                index,
            ),
        ).toBe("batch");
        expect(
            usageDimensionLabel(
                "model_route",
                {
                    model_route_id: 100,
                },
                index,
            ),
        ).toBe("gpt-x");

        const deleted = usageDimensionLabel(
            "provider",
            {
                provider_id: 999,
            },
            index,
        );
        expect(deleted).not.toBe("acme");
        expect(deleted.length).toBeGreaterThan(0);
    });

    test("answers the enum dimensions through their vocabularies", () => {
        expect(
            usageDimensionLabel(
                "endpoint_type",
                {
                    endpoint_type: "openai_responses",
                },
                index,
            ),
        ).not.toBe("openai_responses");
        expect(
            usageDimensionLabel(
                "outcome",
                {
                    outcome: "upstream_error",
                },
                index,
            ),
        ).not.toBe("upstream_error");
    });

    test("answers the stream flag in the operator's words", () => {
        const streamed = usageDimensionLabel(
            "is_stream",
            {
                is_stream: true,
            },
            index,
        );
        const nonstreamed = usageDimensionLabel(
            "is_stream",
            {
                is_stream: false,
            },
            index,
        );

        expect(streamed).not.toBe(nonstreamed);
    });

    test("reads the NULL status bucket as an em dash, a real status as its number", () => {
        expect(
            usageDimensionLabel(
                "http_status",
                {
                    http_status: null,
                },
                index,
            ),
        ).toBe("—");
        expect(
            usageDimensionLabel(
                "http_status",
                {
                    http_status: 429,
                },
                index,
            ),
        ).toBe("429");
    });

    test("asserts a requested dimension's absence instead of rendering past it", () => {
        expect(() => usageDimensionLabel("day", {}, index)).toThrowError(
            "the bucket key carries no day",
        );
    });
});
