import { describe, expect, it } from "vitest";
import { ValidationError } from "/common/errors";
import {
    parseCompleteOAuthInput,
    parseDeleteModelInput,
    parseDiscoverModelsInput,
    parseSaveModelInput,
    parseSaveProviderInput,
    parseStartOAuthInput,
} from "./input";

describe("panel input validation", () => {
    it("normalizes compatible provider URLs without weakening field validation", () => {
        expect(
            parseSaveProviderInput({
                apiKey: "secret",
                baseUrl: "https://example.test/v1///",
                name: "custom",
                protocol: "responses",
                type: "openai_compatible",
            }),
        ).toMatchObject({
            apiKey: "secret",
            baseUrl: "https://example.test/v1",
            name: "custom",
            protocol: "responses",
            type: "openai_compatible",
        });
        expect(() =>
            parseSaveProviderInput({
                name: "custom",
                surprise: true,
            }),
        ).toThrow(ValidationError);
        expect(() =>
            parseSaveProviderInput({
                name: "UPPER",
            }),
        ).toThrow(ValidationError);
        expect(() =>
            parseSaveProviderInput({
                name: "custom",
                enabled: 1,
            }),
        ).toThrow(ValidationError);
    });

    it("preserves omitted fields and distinguishes them from explicit null", () => {
        expect(
            parseSaveProviderInput({
                name: "account",
            }),
        ).toEqual({
            name: "account",
        });
        expect(
            parseSaveModelInput({
                enabled: false,
                providerName: "account",
                thinkingLevels: [
                    "high",
                    "low",
                ],
                upstreamName: "wire-model",
            }),
        ).toEqual({
            enabled: false,
            providerName: "account",
            thinkingLevels: [
                "low",
                "high",
            ],
            upstreamName: "wire-model",
        });
        expect(
            parseSaveModelInput({
                providerName: "account",
                thinkingLevels: null,
                upstreamName: "wire-model",
            }),
        ).toEqual({
            providerName: "account",
            thinkingLevels: null,
            upstreamName: "wire-model",
        });
    });

    it("requires exact OAuth and identifier body contracts", () => {
        expect(
            parseStartOAuthInput({
                type: "openai",
                providerName: "work",
            }),
        ).toEqual({
            providerName: "work",
            type: "openai",
        });
        expect(
            parseCompleteOAuthInput({
                redirectUrl: "http://localhost/callback?code=x&state=y",
                modelPrefix: "oauth/",
            }),
        ).toEqual({
            redirectUrl: "http://localhost/callback?code=x&state=y",
            modelPrefix: "oauth/",
        });
        expect(
            parseDeleteModelInput({
                id: 4,
            }),
        ).toEqual({
            id: 4,
        });
        expect(
            parseDiscoverModelsInput({
                providerName: "work",
            }),
        ).toEqual({
            providerName: "work",
        });
        expect(() =>
            parseDeleteModelInput({
                id: 0,
            }),
        ).toThrow(ValidationError);
        expect(() =>
            parseStartOAuthInput({
                type: "openai_compatible",
            }),
        ).toThrow(ValidationError);
    });
});
