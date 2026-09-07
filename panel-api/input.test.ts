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
                protocol: "openai_response",
                type: "openai_compatible",
            }),
        ).toMatchObject({
            apiKey: "secret",
            baseUrl: "https://example.test/v1",
            kind: "create",
            name: "custom",
            protocol: "openai_response",
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

    it("treats absent model overrides as cleared and validates their shapes", () => {
        expect(
            parseSaveProviderInput({
                name: "account",
            }),
        ).toEqual({
            kind: "update",
            name: "account",
        });
        expect(
            parseSaveModelInput({
                enabledOverride: false,
                nameOverride: "public",
                providerName: "account",
                thinkingLevelsOverride: [
                    "high",
                    "low",
                ],
                upstreamName: "wire-model",
            }),
        ).toEqual({
            enabledOverride: false,
            nameOverride: "public",
            providerName: "account",
            thinkingLevelsOverride: [
                "low",
                "high",
            ],
            upstreamName: "wire-model",
        });
        expect(
            parseSaveModelInput({
                providerName: "account",
                upstreamName: "wire-model",
            }),
        ).toEqual({
            enabledOverride: null,
            nameOverride: null,
            providerName: "account",
            thinkingLevelsOverride: null,
            upstreamName: "wire-model",
        });
        expect(() =>
            parseSaveModelInput({
                nameOverride: "",
                providerName: "account",
                upstreamName: "wire-model",
            }),
        ).toThrow(ValidationError);
        expect(() =>
            parseSaveModelInput({
                enabledOverride: "yes",
                providerName: "account",
                upstreamName: "wire-model",
            }),
        ).toThrow(ValidationError);
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
                projectId: "my-cloud-project",
            }),
        ).toEqual({
            redirectUrl: "http://localhost/callback?code=x&state=y",
            modelPrefix: "oauth/",
            projectId: "my-cloud-project",
        });
        expect(
            parseDeleteModelInput({
                providerName: "work",
                upstreamName: "wire-model",
            }),
        ).toEqual({
            providerName: "work",
            upstreamName: "wire-model",
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
                providerName: "work",
            }),
        ).toThrow(ValidationError);
        expect(() =>
            parseStartOAuthInput({
                type: "openai_compatible",
            }),
        ).toThrow(ValidationError);
    });
});
