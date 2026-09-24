import { describe, expect, it } from "vitest";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
    UsageBucketKey,
} from "/src/management-api/generated/v0";
import {
    indexUsageEntities,
    usageApiKeyLabel,
    usageDimensionLabel,
    usageModelLabel,
    usageProviderLabel,
} from "./usage-labels";

const providers: ReadonlyArray<ProviderResponse> = [
    {
        type: "api_key",
        provider_definition_name: "generic-openai-chat-completions",
        provider_definition_version: 1,
        format: "openai_chat_completion",
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
