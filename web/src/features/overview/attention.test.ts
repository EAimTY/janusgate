import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { ProviderResponse } from "/src/management-api/generated/v0";
import { credentialAttention, inventoryGapAttention } from "./attention";

/**
 * The overview's attention derivations: which providers need a human, why,
 * and the action that answers the cause — an enabled provider with no ready
 * inventory is missing discovery or holds only parked rows, and an OAuth
 * provider's lapsed credential follows the row's parked state for its
 * action.
 */

beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-10-10T12:00:00Z"));
});

afterEach(() => {
    vi.useRealTimers();
});

function provider(
    overrides: Partial<
        Extract<
            ProviderResponse,
            {
                type: "api_key";
            }
        >
    > = {},
): ProviderResponse {
    return {
        type: "api_key",
        id: 1,
        name: "acme",
        model_prefix: "acme",
        is_enabled: true,
        provider_definition_name: "generic-openai-chat-completions",
        provider_definition_version: 1,
        format: "openai_chat_completion",
        base_url: null,
        ...overrides,
    };
}

function oauthProvider(
    overrides: Partial<
        Extract<
            ProviderResponse,
            {
                type: "oauth";
            }
        >
    > = {},
): ProviderResponse {
    return {
        type: "oauth",
        expires_at: null,
        id: 1,
        name: "acme",
        model_prefix: "acme",
        is_enabled: true,
        provider_definition_name: "generic-openai-chat-completions",
        provider_definition_version: 1,
        format: "openai_chat_completion",
        base_url: null,
        ...overrides,
    };
}

describe("inventoryGapAttention", () => {
    test("sends a provider holding no model rows to discovery on its own page", () => {
        const attention = inventoryGapAttention([], 1);

        expect(attention.reason).toBe("no-models");
        expect(attention.action.href).toBe("/providers/1");
    });

    test("sends a provider whose rows exist but none serve to the models page", () => {
        const parked = [
            {
                id: 10,
                provider_id: 1,
                provider: "acme",
                upstream_id: "gpt-x",
                is_enabled: false,
                discovery_state: "discovered",
                thinking_levels: null,
                max_input_tokens: null,
                max_output_tokens: null,
                thinking_levels_override: null,
                max_input_tokens_override: null,
                max_output_tokens_override: null,
                compaction_support: null,
                compaction_support_override: null,
            } as const,
        ];

        const attention = inventoryGapAttention(parked, 1);

        expect(attention.reason).toBe("no-ready-models");
        expect(attention.action.href).toBe("/models?provider=1");
    });
});

describe("credentialAttention", () => {
    test("answers nothing for a static-key provider", () => {
        expect(credentialAttention(provider())).toBeNull();
    });

    test("answers nothing while the OAuth credential still holds", () => {
        const fresh = oauthProvider({
            expires_at: Date.now() * 1000 + 3_600_000_000,
        });

        expect(credentialAttention(fresh)).toBeNull();
    });

    test("sends an enabled provider with an expired credential to the edit page to reconnect", () => {
        const expired = oauthProvider({
            expires_at: Date.now() * 1000 - 3_600_000_000,
        });

        const attention = credentialAttention(expired);

        expect(attention).not.toBeNull();
        if (attention === null) throw new Error("expected attention");
        expect(attention.action.href).toBe("/providers/1/edit");
        expect(attention.detail).toContain("Expired");
    });

    test("counts an OAuth row that never stored an expiry as not fresh", () => {
        const neverFresh = oauthProvider();

        const attention = credentialAttention(neverFresh);

        expect(attention).not.toBeNull();
        if (attention === null) throw new Error("expected attention");
        expect(attention.action.href).toBe("/providers/1/edit");
    });

    test("sends a parked provider to its read-only detail page instead of ordering a reconnect", () => {
        const parked = oauthProvider({
            is_enabled: false,
            expires_at: Date.now() * 1000 - 3_600_000_000,
        });

        const attention = credentialAttention(parked);

        expect(attention).not.toBeNull();
        if (attention === null) throw new Error("expected attention");
        expect(attention.action.href).toBe("/providers/1");
        // The parked state's detail names the credential the re-enable would fail on.
        expect(attention.detail).toContain("Expired");
    });
});
