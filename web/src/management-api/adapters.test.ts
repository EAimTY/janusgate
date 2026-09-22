import { describe, expect, it } from "vitest";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import {
    toCreateModelRequest,
    toUpdateApiKeyRequest,
    toUpdateModelRequest,
    toUpdateProviderRequest,
} from "./adapters";

const compatible: ProviderResponse = {
    type: "openai_chat_completion",
    id: 4,
    name: "local",
    model_prefix: "local/",
    is_enabled: true,
    base_url: "https://upstream.example.com/v1",
};

const codexOauth: ProviderResponse = {
    type: "codex_oauth",
    id: 5,
    name: "chatgpt",
    model_prefix: "gpt-",
    is_enabled: false,
    expires_at: 1_800_000_000,
};

const antigravityOauth: ProviderResponse = {
    type: "antigravity_oauth",
    id: 6,
    name: "antigravity",
    model_prefix: "",
    is_enabled: true,
    expires_at: null,
};

const model: ModelResponse = {
    id: 9,
    provider_id: 4,
    provider: "local",
    upstream_id: "qwen3:32b",
    public_id: "local/qwen3:32b",
    id_override: null,
    is_enabled: true,
    discovery_state: "discovered",
};

describe("toUpdateProviderRequest", () => {
    it("restates the complete readable state and addresses the row by provider_id", () => {
        const request = toUpdateProviderRequest(compatible);

        expect(request).toEqual({
            type: "openai_chat_completion",
            provider_id: 4,
            name: "local",
            model_prefix: "local/",
            is_enabled: true,
            base_url: "https://upstream.example.com/v1",
        });

        // The row's own `id` is a response field; the gateway's strict parsing
        // rejects a body carrying it (README §4.6).
        expect("id" in request).toBe(false);
    });

    it("applies only the edits it is given", () => {
        const request = toUpdateProviderRequest(compatible, {
            name: "renamed",
            base_url: "https://other.example.com",
            is_enabled: false,
        });

        expect(request).toEqual({
            type: "openai_chat_completion",
            provider_id: 4,
            name: "renamed",
            model_prefix: "local/",
            is_enabled: false,
            base_url: "https://other.example.com",
        });
    });

    it("omits the write-only api_key when the secret is kept", () => {
        const request = toUpdateProviderRequest(compatible, {
            name: "renamed",
        });

        expect("api_key" in request).toBe(false);
    });

    it("sends the write-only api_key when the secret is replaced", () => {
        const request = toUpdateProviderRequest(compatible, {
            api_key: "fresh-key",
        });

        expect(request).toMatchObject({
            api_key: "fresh-key",
            name: "local",
        });
    });

    it("maps OAuth variants without a credentials field", () => {
        const openAiRequest = toUpdateProviderRequest(codexOauth, {
            model_prefix: "acct-",
            is_enabled: true,
        });

        expect(openAiRequest).toEqual({
            type: "codex_oauth",
            provider_id: 5,
            name: "chatgpt",
            model_prefix: "acct-",
            is_enabled: true,
        });

        expect("credentials" in openAiRequest).toBe(false);

        const antigravityRequest = toUpdateProviderRequest(antigravityOauth, {
            name: "google",
        });

        expect(antigravityRequest).toEqual({
            type: "antigravity_oauth",
            provider_id: 6,
            name: "google",
            model_prefix: "",
            is_enabled: true,
        });

        expect("credentials" in antigravityRequest).toBe(false);
        // The OAuth variants carry no upstream fields and no expiry in a request.
        expect("expires_at" in antigravityRequest).toBe(false);
        expect("base_url" in antigravityRequest).toBe(false);
    });
});

describe("toUpdateModelRequest", () => {
    it("restates the row's two writable columns and addresses it by model_id", () => {
        expect(toUpdateModelRequest(model)).toEqual({
            model_id: 9,
            id_override: null,
            is_enabled: true,
        });
    });

    it("applies a single field change while restating the rest", () => {
        expect(
            toUpdateModelRequest(model, {
                is_enabled: false,
            }),
        ).toEqual({
            model_id: 9,
            id_override: null,
            is_enabled: false,
        });
    });

    it("keeps an explicit null as the no-override state", () => {
        const overridden: ModelResponse = {
            ...model,
            id_override: "public-id",
        };

        expect(
            toUpdateModelRequest(overridden, {
                id_override: null,
            }),
        ).toEqual({
            model_id: 9,
            id_override: null,
            is_enabled: true,
        });
    });

    it("never addresses the row by its natural key or its public routing id", () => {
        const request = toUpdateModelRequest(model, {
            id_override: "renamed-id",
        });

        expect(request.model_id).toBe(9);
        expect("provider_id" in request).toBe(false);
        expect("upstream_id" in request).toBe(false);
        expect("public_id" in request).toBe(false);
        expect("discovery_state" in request).toBe(false);
    });
});

const apiKey: ApiKeyResponse = {
    id: 7,
    name: "service-a",
    key: "sk-live",
    is_enabled: true,
    created_at: 1_700_000_000_000_000,
};

describe("toUpdateApiKeyRequest", () => {
    it("restates the complete row state and addresses it by api_key_id", () => {
        expect(toUpdateApiKeyRequest(apiKey)).toEqual({
            api_key_id: 7,
            name: "service-a",
            key: "sk-live",
            is_enabled: true,
        });
    });

    it("applies only the edits it is given", () => {
        expect(
            toUpdateApiKeyRequest(apiKey, {
                name: "renamed",
                is_enabled: false,
            }),
        ).toEqual({
            api_key_id: 7,
            name: "renamed",
            key: "sk-live",
            is_enabled: false,
        });
    });

    it("never carries the server-assigned created_at", () => {
        expect("created_at" in toUpdateApiKeyRequest(apiKey)).toBe(false);
        expect("id" in toUpdateApiKeyRequest(apiKey)).toBe(false);
    });
});

describe("toCreateModelRequest", () => {
    it("states the complete initial row for a fresh natural key", () => {
        expect(toCreateModelRequest(4, "gpt-4o")).toEqual({
            provider_id: 4,
            upstream_id: "gpt-4o",
            id_override: null,
            is_enabled: true,
        });
    });

    it("carries the two writable columns explicitly", () => {
        expect(
            toCreateModelRequest(4, "gpt-4o", {
                id_override: "custom",
                is_enabled: false,
            }),
        ).toEqual({
            provider_id: 4,
            upstream_id: "gpt-4o",
            id_override: "custom",
            is_enabled: false,
        });
    });
});
