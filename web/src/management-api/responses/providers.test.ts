import { describe, expect, test } from "vitest";
import { parseListProvidersResponse, parseProviderEnvelope } from "./providers";

/**
 * The provider response parsers: the credential kind resolved first at the
 * discriminator, the shared definition pair and derived format, and the
 * OAuth member alone carrying the nullable expiry — with a wrong-kind body
 * named at `type` before any shared field can fail first.
 */

/** One OAuth provider row fixture, expiry still ahead. */
const OAUTH_ROW = {
    type: "oauth",
    id: 2,
    name: "acme",
    model_prefix: "acme",
    is_enabled: true,
    provider_definition_name: "claude-code",
    provider_definition_version: 1,
    format: "anthropic_messages",
    base_url: null,
    expires_at: 1_800_000_000_000_000,
};

/** One static-key provider row fixture. */
const API_KEY_ROW = {
    type: "api_key",
    id: 9,
    name: "direct",
    model_prefix: "",
    is_enabled: false,
    provider_definition_name: "generic-openai-chat-completions",
    provider_definition_version: 4,
    format: "openai_chat_completion",
    base_url: "https://relay.example.test/v1",
};

describe("provider response parsers", () => {
    test("parse the listing across both credential kinds", () => {
        const listing = parseListProvidersResponse({
            providers: [
                OAUTH_ROW,
                API_KEY_ROW,
            ],
        });

        expect(listing.providers).toHaveLength(2);
        const oauth = listing.providers[0];
        if (oauth === undefined || oauth.type !== "oauth") {
            throw new Error("the fixture's first row must be the OAuth member");
        }
        expect(oauth.expires_at).toBe(1_800_000_000_000_000);
        const apiKey = listing.providers[1];
        if (apiKey === undefined || apiKey.type !== "api_key") {
            throw new Error("the fixture's second row must be the static-key member");
        }
        // The static-key member carries no expiry at all.
        expect(apiKey).not.toHaveProperty("expires_at");
    });

    test("parse the creation and rebind envelopes", () => {
        expect(
            parseProviderEnvelope({
                provider: API_KEY_ROW,
            }).provider.id,
        ).toBe(9);
    });

    test("name a wrong-kind body at the discriminator", () => {
        expect(() =>
            parseProviderEnvelope({
                provider: {
                    ...OAUTH_ROW,
                    type: "token",
                },
            }),
        ).toThrowError("response.provider.type is not a known credential kind: token");
    });

    test("reject a wrong format and an out-of-range definition version", () => {
        expect(() =>
            parseProviderEnvelope({
                provider: {
                    ...OAUTH_ROW,
                    format: "graphql",
                },
            }),
        ).toThrowError("response.provider.format is not an upstream format: graphql");
        expect(() =>
            parseProviderEnvelope({
                provider: {
                    ...API_KEY_ROW,
                    provider_definition_version: 0,
                },
            }),
        ).toThrowError("must be a safe integer of at least 1");
    });
});
