import { describe, expect, test } from "vitest";
import {
    toCreateModelRequest,
    toUpdateApiKeyRequest,
    toUpdateModelRequest,
    toUpdateProviderRequest,
} from "./adapters";
import type { ApiKeyResponse, ModelResponse, ProviderResponse } from "./generated/v0";

/**
 * The read-modify-write edit adapters: the complete desired state each
 * write payload restates from a listed row, the write-only `api_key` kept
 * when absent, the explicit `null` that clears a nullable mirror carried as
 * `null` — never silently falling back to the stored value — and the row's
 * own identity facts never traveling in a request.
 */

/** One static-key provider row. */
const PROVIDER: ProviderResponse = {
    type: "api_key",
    id: 11,
    name: "main",
    model_prefix: "main",
    is_enabled: true,
    provider_definition_name: "generic-openai-responses",
    provider_definition_version: 3,
    format: "openai_chat_completion",
    base_url: "https://api.example.test/v1",
};

/** One model row with discovered facts and standing overrides. */
const MODEL: ModelResponse = {
    id: 21,
    provider_id: 11,
    provider: "main",
    upstream_id: "gpt-x",
    is_enabled: true,
    discovery_state: "discovered",
    thinking_levels: [
        "low",
        "high",
    ],
    max_input_tokens: 100_000,
    max_output_tokens: 4_000,
    thinking_levels_override: null,
    max_input_tokens_override: null,
    max_output_tokens_override: 8_000,
    compaction_support: {
        responses_standalone_compact: "supported",
    },
    compaction_support_override: null,
};

/** One serving-key row. */
const API_KEY: ApiKeyResponse = {
    id: 31,
    name: "batch job",
    key: "jkc-secret-1",
    is_enabled: true,
    created_at: 1_700_000_000_000_000,
    access_control: {
        default_access: "deny",
        owner_defaults: [],
        route_overrides: [],
    },
};

describe("toUpdateProviderRequest", () => {
    test("restates the row's readable state when no edit is given", () => {
        expect(toUpdateProviderRequest(PROVIDER)).toEqual({
            provider_id: 11,
            name: "main",
            model_prefix: "main",
            is_enabled: true,
            base_url: "https://api.example.test/v1",
        });
    });

    test("carries an explicit null base URL as the wire null, not the stored one", () => {
        const request = toUpdateProviderRequest(PROVIDER, {
            base_url: null,
        });

        expect(request.base_url).toBeNull();
    });

    test("keeps the stored base URL when the edit omits the field", () => {
        expect(
            toUpdateProviderRequest(PROVIDER, {
                name: "renamed",
            }).base_url,
        ).toBe("https://api.example.test/v1");
    });

    test("carries a replacement static key only when the edit states one", () => {
        expect(
            toUpdateProviderRequest(PROVIDER, {
                api_key: "sk-new",
            }),
        ).toEqual({
            provider_id: 11,
            name: "main",
            model_prefix: "main",
            is_enabled: true,
            base_url: "https://api.example.test/v1",
            api_key: "sk-new",
        });

        // The stored secret is write-only and never read back, so the default
        // request never names `api_key` at all.
        expect(toUpdateProviderRequest(PROVIDER)).not.toHaveProperty("api_key");
    });

    test("never carries the row's id as a name-side field or the definition pair", () => {
        const request = toUpdateProviderRequest(PROVIDER);

        expect(request).not.toHaveProperty("id");
        expect(request).not.toHaveProperty("provider_definition_name");
        expect(request).not.toHaveProperty("provider_definition_version");
        expect(request).not.toHaveProperty("type");
    });
});

describe("toUpdateModelRequest", () => {
    test("restates every writable column, overrides and bases alike", () => {
        expect(toUpdateModelRequest(MODEL)).toEqual({
            model_id: 21,
            is_enabled: true,
            thinking_levels_override: null,
            max_input_tokens_override: null,
            max_output_tokens_override: 8_000,
            compaction_support_override: null,
        });
    });

    test("carries an explicit null override as the null that keeps the discovered value", () => {
        const request = toUpdateModelRequest(
            {
                ...MODEL,
                max_output_tokens_override: 8_000,
            },
            {
                max_output_tokens_override: null,
            },
        );

        expect(request.max_output_tokens_override).toBeNull();
    });

    test("replaces an override wholesale, the empty level array included", () => {
        const request = toUpdateModelRequest(MODEL, {
            thinking_levels_override: [],
        });

        expect(request.thinking_levels_override).toEqual([]);
    });

    test("addresses the row by its id and never by its natural key", () => {
        const request = toUpdateModelRequest(MODEL, {
            is_enabled: false,
        });

        expect(request.model_id).toBe(21);
        expect(request.is_enabled).toBe(false);
        expect(request).not.toHaveProperty("upstream_id");
        expect(request).not.toHaveProperty("discovery_state");
        expect(request).not.toHaveProperty("thinking_levels");
    });
});

describe("toUpdateApiKeyRequest", () => {
    test("restates the complete desired state, readable secret included", () => {
        expect(
            toUpdateApiKeyRequest(API_KEY, {
                name: "renamed",
            }),
        ).toEqual({
            api_key_id: 31,
            name: "renamed",
            key: "jkc-secret-1",
            is_enabled: true,
            access_control: {
                default_access: "deny",
                owner_defaults: [],
                route_overrides: [],
            },
        });
    });

    test("never carries the server-assigned id and creation timestamp as fields", () => {
        const request = toUpdateApiKeyRequest(API_KEY);

        expect(request).not.toHaveProperty("id");
        expect(request).not.toHaveProperty("created_at");
    });
});

describe("toCreateModelRequest", () => {
    test("states the natural key and defaults every override to null, enabled", () => {
        expect(
            toCreateModelRequest(11, "gpt-x", {
                max_input_tokens_override: 32_000,
            }),
        ).toEqual({
            provider_id: 11,
            upstream_id: "gpt-x",
            is_enabled: true,
            thinking_levels_override: null,
            max_input_tokens_override: 32_000,
            max_output_tokens_override: null,
            compaction_support_override: null,
        });
    });

    test("carries an explicit is_enabled override over the default", () => {
        expect(
            toCreateModelRequest(11, "gpt-x", {
                is_enabled: false,
            }).is_enabled,
        ).toBe(false);
    });

    test("carries a compaction override object verbatim, the explicit unknown included", () => {
        const request = toCreateModelRequest(11, "gpt-x", {
            compaction_support_override: {
                messages_on_demand_compaction: "unknown",
            },
        });

        expect(request.compaction_support_override).toEqual({
            messages_on_demand_compaction: "unknown",
        });
    });
});
