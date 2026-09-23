import { describe, expect, it } from "vitest";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
    UsageBucketKey,
} from "/src/management-api/generated/v0";
import {
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

describe("usage labels", () => {
    it("resolves a listed id through the entity lists", () => {
        expect(usageModelLabel(entities, 9)).toEqual({
            text: "local-qwen3-32b",
            isListed: true,
        });
        expect(usageProviderLabel(entities, 1)).toEqual({
            text: "local",
            isListed: true,
        });
        expect(usageApiKeyLabel(entities, 7)).toEqual({
            text: "service-a",
            isListed: true,
        });
    });

    it("names a deleted row by its kind, never by a guessed name or number", () => {
        expect(usageModelLabel(entities, 11)).toEqual({
            text: "Deleted model",
            isListed: false,
        });
        expect(usageProviderLabel(entities, 3)).toEqual({
            text: "Deleted provider",
            isListed: false,
        });
        expect(usageApiKeyLabel(entities, 8)).toEqual({
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

        expect(usageDimensionLabel("day", key, entities)).toBe("2026-09-01");
        expect(usageDimensionLabel("model", key, entities)).toBe("local-qwen3-32b");
        expect(usageDimensionLabel("provider", key, entities)).toBe("local");
        expect(usageDimensionLabel("api_key", key, entities)).toBe("service-a");
        expect(usageDimensionLabel("endpoint_type", key, entities)).toBe("Responses");
        expect(usageDimensionLabel("outcome", key, entities)).toBe("Client aborted");
        expect(usageDimensionLabel("is_stream", key, entities)).toBe("Streamed");
        // The `NULL` status bucket is a real bucket: an em dash, never a crash.
        expect(usageDimensionLabel("http_status", key, entities)).toBe("—");
        expect(
            usageDimensionLabel(
                "http_status",
                {
                    ...key,
                    http_status: 429,
                },
                entities,
            ),
        ).toBe("429");
        expect(
            usageDimensionLabel(
                "is_stream",
                {
                    ...key,
                    is_stream: false,
                },
                entities,
            ),
        ).toBe("Non-streamed");
    });

    it("treats a key field missing for a requested dimension as a contract violation", () => {
        expect(() => usageDimensionLabel("day", {}, entities)).toThrow(
            /the bucket key carries no day/,
        );
        expect(() =>
            usageDimensionLabel(
                "model",
                {
                    day: "2026-09-01",
                },
                entities,
            ),
        ).toThrow(/the bucket key carries no model_id/);
    });
});
