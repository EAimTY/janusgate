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
    type: "api_key",
    id: 4,
    name: "local",
    model_prefix: "local-",
    is_enabled: true,
    provider_definition_name: "generic-openai-chat-completions",
    provider_definition_version: 1,
    format: "openai_chat_completion",
    base_url: "https://upstream.example.com/v1",
};

const codexOauth: ProviderResponse = {
    type: "oauth",
    id: 5,
    name: "chatgpt",
    model_prefix: "gpt-",
    is_enabled: false,
    provider_definition_name: "codex",
    provider_definition_version: 1,
    format: "openai_response",
    base_url: null,
    expires_at: 1_800_000_000_000_000,
};

const antigravityOauth: ProviderResponse = {
    type: "oauth",
    id: 6,
    name: "antigravity",
    model_prefix: "",
    is_enabled: true,
    provider_definition_name: "antigravity",
    provider_definition_version: 1,
    format: "gemini_generate_content",
    base_url: null,
    expires_at: null,
};

const model: ModelResponse = {
    id: 9,
    provider_id: 4,
    provider: "local",
    upstream_id: "qwen3:32b",
    public_id: "local-qwen3-32b",
    id_override: null,
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
};

describe("toUpdateProviderRequest", () => {
    it("restates the complete readable state and addresses the row by provider_id", () => {
        const request = toUpdateProviderRequest(compatible);

        expect(request).toEqual({
            provider_id: 4,
            name: "local",
            model_prefix: "local-",
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
            provider_id: 4,
            name: "renamed",
            model_prefix: "local-",
            is_enabled: false,
            base_url: "https://other.example.com",
        });
    });

    it("sends a present null base_url edit instead of the stored URL", () => {
        // `base_url` is a two-state edit: absent keeps the stored value, but a
        // present `null` is the value to send — a legal full-state write
        // whenever the effective definition does not reference the base
        // (gateway README §7.1) — so the adapter must not collapse it onto the
        // stored URL.
        const request = toUpdateProviderRequest(compatible, {
            base_url: null,
        });

        expect(request.base_url).toBe(null);
    });

    it("restates a typed prefix byte-for-byte, trailing dash included", () => {
        // The gateway stores `model_prefix` verbatim and recomposes every
        // unoverridden public ID from exactly what the request states
        // (gateway README §7) — a prefix trimmed here would rewrite every
        // client's routing, so no edit value may ever be normalized.
        const request = toUpdateProviderRequest(compatible, {
            model_prefix: "q-",
        });

        expect(request.model_prefix).toBe("q-");
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

    it("restates the nullable base URL and never carries the expiry", () => {
        const openAiRequest = toUpdateProviderRequest(codexOauth, {
            model_prefix: "acct-",
            is_enabled: true,
        });

        expect(openAiRequest).toEqual({
            provider_id: 5,
            name: "chatgpt",
            model_prefix: "acct-",
            is_enabled: true,
            base_url: null,
        });

        expect("credentials" in openAiRequest).toBe(false);
        // The request carries no expiry: it is the flow's product, not an input.
        expect("expires_at" in openAiRequest).toBe(false);

        const antigravityRequest = toUpdateProviderRequest(antigravityOauth, {
            name: "google",
        });

        expect(antigravityRequest).toEqual({
            provider_id: 6,
            name: "google",
            model_prefix: "",
            is_enabled: true,
            base_url: null,
        });

        expect("expires_at" in antigravityRequest).toBe(false);
    });
});

describe("toUpdateModelRequest", () => {
    it("restates the row's writable columns and addresses it by model_id", () => {
        expect(toUpdateModelRequest(model)).toEqual({
            model_id: 9,
            id_override: null,
            is_enabled: true,
            thinking_levels_override: null,
            max_input_tokens_override: null,
            max_output_tokens_override: null,
            compaction_support_override: null,
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
            thinking_levels_override: null,
            max_input_tokens_override: null,
            max_output_tokens_override: null,
            compaction_support_override: null,
        });
    });

    it("applies capability overrides explicitly, including clearing them", () => {
        const overridden: ModelResponse = {
            ...model,
            thinking_levels_override: [
                "low",
            ],
            max_input_tokens_override: 200_000,
            max_output_tokens_override: 32_768,
        };

        // A present value replaces the declaration wholesale.
        expect(
            toUpdateModelRequest(model, {
                thinking_levels_override: [
                    "low",
                    "high",
                ],
                max_input_tokens_override: 200_000,
            }),
        ).toEqual({
            model_id: 9,
            id_override: null,
            is_enabled: true,
            thinking_levels_override: [
                "low",
                "high",
            ],
            max_input_tokens_override: 200_000,
            max_output_tokens_override: null,
            compaction_support_override: null,
        });

        // An explicit `null` clears the override back to the discovered value.
        expect(
            toUpdateModelRequest(overridden, {
                thinking_levels_override: null,
                max_input_tokens_override: null,
                max_output_tokens_override: null,
            }),
        ).toEqual({
            model_id: 9,
            id_override: null,
            is_enabled: true,
            thinking_levels_override: null,
            max_input_tokens_override: null,
            max_output_tokens_override: null,
            compaction_support_override: null,
        });
    });

    it("applies the compaction-facts override explicitly, including clearing it", () => {
        const overridden: ModelResponse = {
            ...model,
            compaction_support: {
                messages_on_demand_compaction: "supported",
            },
            compaction_support_override: {
                messages_on_demand_compaction: "unknown",
                messages_threshold_compaction: "supported",
            },
        };

        // A present map replaces the discovered facts wholesale — the
        // explicit unknown member included.
        expect(
            toUpdateModelRequest(model, {
                compaction_support_override: {
                    messages_on_demand_compaction: "unknown",
                },
            }).compaction_support_override,
        ).toEqual({
            messages_on_demand_compaction: "unknown",
        });

        // An omitted edit restates the row's standing override.
        expect(toUpdateModelRequest(overridden).compaction_support_override).toEqual({
            messages_on_demand_compaction: "unknown",
            messages_threshold_compaction: "supported",
        });

        // An explicit `null` clears the override back to the discovered
        // facts.
        expect(
            toUpdateModelRequest(overridden, {
                compaction_support_override: null,
            }).compaction_support_override,
        ).toBe(null);
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
            thinking_levels_override: null,
            max_input_tokens_override: null,
            max_output_tokens_override: null,
            compaction_support_override: null,
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
            thinking_levels_override: null,
            max_input_tokens_override: null,
            max_output_tokens_override: null,
            compaction_support_override: null,
        });
    });

    it("carries the writable columns explicitly", () => {
        expect(
            toCreateModelRequest(4, "gpt-4o", {
                id_override: "custom",
                is_enabled: false,
                thinking_levels_override: [
                    "low",
                ],
                max_input_tokens_override: 200_000,
                max_output_tokens_override: 32_768,
                compaction_support_override: {
                    messages_on_demand_compaction: "supported",
                },
            }),
        ).toEqual({
            provider_id: 4,
            upstream_id: "gpt-4o",
            id_override: "custom",
            is_enabled: false,
            thinking_levels_override: [
                "low",
            ],
            max_input_tokens_override: 200_000,
            max_output_tokens_override: 32_768,
            compaction_support_override: {
                messages_on_demand_compaction: "supported",
            },
        });
    });
});
