import { describe, expect, it } from "vitest";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { usageApiKeyLabel, usageModelLabel, usageProviderLabel } from "./usage-labels";

const providers: ReadonlyArray<ProviderResponse> = [
    {
        type: "openai_chat_completion",
        id: 1,
        name: "local",
        model_prefix: "local/",
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
        public_id: "local/qwen3:32b",
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
            text: "local/qwen3:32b",
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
});
