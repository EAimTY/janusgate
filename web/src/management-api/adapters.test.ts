import { describe, expect, it } from "vitest";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { toUpdateModelRequest, toUpdateProviderRequest } from "./adapters";

const compatible: ProviderResponse = {
    type: "openai_compatible",
    id: 4,
    name: "local",
    model_prefix: "local/",
    is_enabled: true,
    base_url: "https://upstream.example.com/v1",
    protocol: "openai_chat_completion",
};

const openaiOauth: ProviderResponse = {
    type: "openai_oauth",
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
    provider_id: 4,
    provider: "local",
    upstream_id: "qwen3:32b",
    id: "local/qwen3:32b",
    id_override: null,
    is_enabled: true,
    is_effectively_enabled: true,
    thinking_levels_override: [
        "low",
    ],
};

describe("toUpdateProviderRequest", () => {
    it("restates the complete readable state and addresses the row by provider_id", () => {
        const request = toUpdateProviderRequest(compatible);

        expect(request).toEqual({
            type: "openai_compatible",
            provider_id: 4,
            name: "local",
            model_prefix: "local/",
            is_enabled: true,
            base_url: "https://upstream.example.com/v1",
            protocol: "openai_chat_completion",
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
            type: "openai_compatible",
            provider_id: 4,
            name: "renamed",
            model_prefix: "local/",
            is_enabled: false,
            base_url: "https://other.example.com",
            protocol: "openai_chat_completion",
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
        const openAiRequest = toUpdateProviderRequest(openaiOauth, {
            model_prefix: "acct-",
            is_enabled: true,
        });

        expect(openAiRequest).toEqual({
            type: "openai_oauth",
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
    it("restates the row's complete override state", () => {
        expect(toUpdateModelRequest(model)).toEqual({
            provider_id: 4,
            upstream_id: "qwen3:32b",
            id_override: null,
            is_enabled: true,
            thinking_levels_override: [
                "low",
            ],
        });
    });

    it("applies a single field change while restating the rest", () => {
        expect(
            toUpdateModelRequest(model, {
                is_enabled: false,
            }),
        ).toEqual({
            provider_id: 4,
            upstream_id: "qwen3:32b",
            id_override: null,
            is_enabled: false,
            thinking_levels_override: [
                "low",
            ],
        });
    });

    it("keeps an explicit null as the no-override state", () => {
        const overridden: ModelResponse = {
            ...model,
            id_override: "public-id",
            thinking_levels_override: null,
        };

        expect(
            toUpdateModelRequest(overridden, {
                id_override: null,
            }),
        ).toMatchObject({
            id_override: null,
            thinking_levels_override: null,
        });
    });

    it("canonicalizes thinking levels into the contract's order", () => {
        const request = toUpdateModelRequest(model, {
            thinking_levels_override: [
                "high",
                "off",
                "high",
                "medium",
            ],
        });

        expect(request.thinking_levels_override).toEqual([
            "off",
            "medium",
            "high",
        ]);
    });

    it("never addresses the row by its public routing id", () => {
        const request = toUpdateModelRequest(model, {
            id_override: "renamed-id",
        });

        expect(request.provider_id).toBe(4);
        expect(request.upstream_id).toBe("qwen3:32b");
        expect("id" in request).toBe(false);
        expect("provider" in request).toBe(false);
    });
});
