import { describe, expect, test } from "vitest";
import type { ProviderResponse } from "/src/management-api/generated/v0";
import { apiKeySavedNotice } from "../api-keys/notices";
import { credentialBadge } from "./credential-presentation";
import { suggestProviderName } from "./provider-defaults";

/**
 * The providers' small derivations: the first free name for a fresh provider
 * (gaps reused from the lowest suffix up), the serving-key save notice
 * composing its two consequence lines, and the credential badge read off the
 * single standing derivation — a static key earning no badge at all.
 */

describe("suggestProviderName", () => {
    test("answers the base name when it is free", () => {
        expect(suggestProviderName("acme", new Set())).toBe("acme");
        expect(
            suggestProviderName(
                "acme",
                new Set([
                    "other",
                ]),
            ),
        ).toBe("acme");
    });

    test("suffixes from 1 up while the base and its suffixes are taken", () => {
        expect(
            suggestProviderName(
                "acme",
                new Set([
                    "acme",
                ]),
            ),
        ).toBe("acme-1");
        expect(
            suggestProviderName(
                "acme",
                new Set([
                    "acme",
                    "acme-1",
                    "acme-2",
                ]),
            ),
        ).toBe("acme-3");
    });

    test("reuses the lowest gap a rename left behind", () => {
        expect(
            suggestProviderName(
                "acme",
                new Set([
                    "acme",
                    "acme-1",
                    "acme-3",
                ]),
            ),
        ).toBe("acme-2");
    });
});

describe("apiKeySavedNotice", () => {
    test("states the rotation's consequence beside the saved name when the secret changed", () => {
        const notice = apiKeySavedNotice({
            name: "batch",
            isRotatingSecret: true,
            isTurningOffLastEnabled: false,
        });

        expect(notice.tone).toBe("ok");
        // The title is the rich saved-name message; assert it states one, the
        // JSX slot's rendering being the shell's business.
        expect(notice.title).toBeTruthy();
        expect(notice.detail).not.toBeNull();
    });

    test("composes the last-enabled line when the same save turns off the last enabled key", () => {
        const both = apiKeySavedNotice({
            name: "batch",
            isRotatingSecret: true,
            isTurningOffLastEnabled: true,
        });
        const lastOnly = apiKeySavedNotice({
            name: "batch",
            isRotatingSecret: false,
            isTurningOffLastEnabled: true,
        });

        const bothDetail = String(both.detail);
        const lastDetail = String(lastOnly.detail);
        expect(bothDetail).toContain(lastDetail);
    });

    test("carries no detail for a save with neither consequence", () => {
        const notice = apiKeySavedNotice({
            name: "batch",
            isRotatingSecret: false,
            isTurningOffLastEnabled: false,
        });

        expect(notice.detail).toBeNull();
    });
});

describe("credentialBadge", () => {
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
            provider_definition_name: "d",
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
            provider_definition_name: "d",
            provider_definition_version: 1,
            format: "openai_chat_completion",
            base_url: null,
            ...overrides,
        };
    }

    test("answers no badge for a static key, which never expires", () => {
        expect(credentialBadge(provider())).toBeNull();
    });

    test("answers the fresh, expired, and never-fresh badges with their tones", () => {
        const ahead = Date.now() * 1000 + 3_600_000_000;
        const behind = Date.now() * 1000 - 3_600_000_000;

        expect(
            credentialBadge(
                oauthProvider({
                    expires_at: ahead,
                }),
            )?.tone,
        ).toBe("ok");
        expect(
            credentialBadge(
                oauthProvider({
                    expires_at: behind,
                }),
            )?.tone,
        ).toBe("warn");
        expect(
            credentialBadge(
                oauthProvider({
                    expires_at: null,
                }),
            )?.tone,
        ).toBe("warn");
        // The never-fresh badge carries the tooltip the bare word nowhere explains.
        expect(
            credentialBadge(
                oauthProvider({
                    expires_at: null,
                }),
            )?.title,
        ).toBeTruthy();
    });
});
