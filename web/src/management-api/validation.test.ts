import { describe, expect, it } from "vitest";
import { ContractViolationError } from "./errors";

import {
    parseDiscoverModelsResponse,
    parseEmptyResponse,
    parseErrorResponse,
    parseListModelsResponse,
    parseListProvidersResponse,
    parseModelEnvelope,
    parseProviderEnvelope,
    parseReauthorizeProviderResponse,
    parseStartProviderOauthResponse,
} from "./validation";

const compatibleProvider = {
    type: "openai_compatible",
    id: 1,
    name: "local",
    model_prefix: "local/",
    is_enabled: true,
    base_url: "https://upstream.example.com",
    protocol: "openai_chat_completion",
};

const oauthProvider = {
    type: "openai_oauth",
    id: 2,
    name: "chatgpt",
    model_prefix: "",
    is_enabled: false,
    expires_at: 1_800_000_000,
};

const antigravityProvider = {
    type: "antigravity_oauth",
    id: 3,
    name: "antigravity",
    model_prefix: "ag/",
    is_enabled: true,
    expires_at: null,
};

const model = {
    provider_id: 1,
    provider: "local",
    upstream_id: "qwen3:32b",
    id: "local/qwen3:32b",
    id_override: null,
    is_enabled: true,
    is_effectively_enabled: true,
    thinking_levels_override: null,
};

describe("response validation", () => {
    it("parses every provider variant", () => {
        const response = parseListProvidersResponse({
            providers: [
                compatibleProvider,
                oauthProvider,
                antigravityProvider,
            ],
        });

        expect(response.providers).toEqual([
            compatibleProvider,
            oauthProvider,
            antigravityProvider,
        ]);
    });

    it("tolerates unknown extra keys but reads every required one", () => {
        const response = parseListProvidersResponse({
            providers: [
                {
                    ...compatibleProvider,
                    future_field: "ignored",
                },
            ],
            future_envelope_field: 7,
        });

        expect(response.providers).toEqual([
            compatibleProvider,
        ]);
    });

    it("rejects a provider that is missing a field of its variant", () => {
        const { base_url: _baseUrl, ...withoutBaseUrl } = compatibleProvider;

        expect(() =>
            parseListProvidersResponse({
                providers: [
                    withoutBaseUrl,
                ],
            }),
        ).toThrow(/response\.providers\[0\]\.base_url must be a string/);
    });

    it("rejects a provider whose type is not part of the union", () => {
        expect(() =>
            parseListProvidersResponse({
                providers: [
                    {
                        ...compatibleProvider,
                        type: "gemini_oauth",
                    },
                ],
            }),
        ).toThrow(/response\.providers\[0\]\.type is not a known provider type/);
    });

    it("rejects a non-object response and a non-array listing", () => {
        expect(() => parseListProvidersResponse("nope")).toThrow(ContractViolationError);

        expect(() =>
            parseListProvidersResponse({
                providers: {},
            }),
        ).toThrow(/response\.providers must be an array/);
    });

    it("keeps the two nullable mirror columns of a model", () => {
        const overridden = {
            ...model,
            id_override: "custom-id",
            thinking_levels_override: [
                "low",
                "high",
            ],
            is_effectively_enabled: false,
        };

        const response = parseListModelsResponse({
            models: [
                model,
                overridden,
            ],
        });

        expect(response.models).toEqual([
            model,
            overridden,
        ]);
    });

    it("rejects a thinking level that is not canonical", () => {
        expect(() =>
            parseListModelsResponse({
                models: [
                    {
                        ...model,
                        thinking_levels_override: [
                            "medium",
                            "extreme",
                        ],
                    },
                ],
            }),
        ).toThrow(
            /response\.models\[0\]\.thinking_levels_override\[1\] is not a canonical thinking level/,
        );
    });

    it("rejects an omitted required-nullable field", () => {
        const { id_override: _idOverride, ...withoutOverride } = model;

        expect(() =>
            parseListModelsResponse({
                models: [
                    withoutOverride,
                ],
            }),
        ).toThrow(/response\.models\[0\]\.id_override must be a string or null/);
    });

    it("parses envelopes and discovery listings", () => {
        expect(
            parseProviderEnvelope({
                provider: oauthProvider,
            }),
        ).toEqual({
            provider: oauthProvider,
        });

        expect(
            parseModelEnvelope({
                model,
            }),
        ).toEqual({
            model,
        });

        expect(
            parseDiscoverModelsResponse({
                models: [
                    model,
                ],
            }),
        ).toEqual({
            models: [
                model,
            ],
        });

        expect(
            parseStartProviderOauthResponse({
                authorize_url: "https://auth.example.com/authorize",
                state: "opaque",
                expires_at: 1_800_000_600,
            }),
        ).toEqual({
            authorize_url: "https://auth.example.com/authorize",
            state: "opaque",
            expires_at: 1_800_000_600,
        });

        expect(
            parseReauthorizeProviderResponse({
                expires_at: 1_800_000_600,
            }),
        ).toEqual({
            expires_at: 1_800_000_600,
        });
    });

    it("accepts only an object as an empty response", () => {
        expect(parseEmptyResponse({})).toEqual({});
        expect(() => parseEmptyResponse([])).toThrow(/response must be an object/);
        expect(() => parseEmptyResponse(null)).toThrow(ContractViolationError);
    });
});

describe("parseErrorResponse", () => {
    it("reads the flat errno-shaped body", () => {
        expect(
            parseErrorResponse({
                errno: 2,
                message: "invalid management token",
            }),
        ).toEqual({
            errno: 2,
            message: "invalid management token",
        });
    });

    it("rejects bodies outside the closed taxonomy and malformed bodies", () => {
        expect(
            parseErrorResponse({
                errno: 99,
                message: "unknown",
            }),
        ).toBeNull();

        expect(
            parseErrorResponse({
                errno: 2,
            }),
        ).toBeNull();

        expect(
            parseErrorResponse({
                errno: "2",
                message: "text errno",
            }),
        ).toBeNull();

        expect(parseErrorResponse("plain text")).toBeNull();
        expect(parseErrorResponse(null)).toBeNull();
    });
});
