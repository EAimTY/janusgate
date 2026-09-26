import { describe, expect, it } from "vitest";
import { nowMicros } from "/src/format";
import type { ProviderResponse } from "/src/management-api/generated/v0";
import { credentialBadge } from "/src/ui/providers-table";

const staticKeyProvider: ProviderResponse = {
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

/** The OAuth variant of the same row, carrying the expiry under test. */
function oauthProvider(expiresAt: number | null): ProviderResponse {
    return {
        type: "oauth",
        id: 5,
        name: "cloud",
        model_prefix: "cloud-",
        is_enabled: true,
        provider_definition_name: "openai",
        provider_definition_version: 1,
        format: "openai_response",
        base_url: null,
        expires_at: expiresAt,
    };
}

describe("credentialBadge", () => {
    const now = nowMicros();

    it("gives a static key no badge: its kind is named on the row's own line and it never expires", () => {
        expect(credentialBadge(staticKeyProvider)).toBeNull();
    });

    it("reads an OAuth login with no stored expiry as Stale", () => {
        expect(credentialBadge(oauthProvider(null))).toEqual({
            tone: "warn",
            label: "Stale",
        });
    });

    it("reads an OAuth login past its expiry as Expired", () => {
        expect(credentialBadge(oauthProvider(now - 3_600 * 1_000_000))).toEqual({
            tone: "warn",
            label: "Expired",
        });
    });

    it("reads an OAuth login within its expiry as Valid", () => {
        expect(credentialBadge(oauthProvider(now + 3_600 * 1_000_000))).toEqual({
            tone: "ok",
            label: "Valid",
        });
    });
});
