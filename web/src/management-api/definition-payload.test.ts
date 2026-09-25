import { describe, expect, it } from "vitest";
import {
    parseListProviderDefinitionsResponse,
    validateDefinitionPayload,
} from "./definition-payload";
import { PROVIDER_PRESETS } from "./presets";

/** One minimal stored definition row every field the app reads, plus its payload. */
const storedDefinition = {
    name: "generic-openai-responses",
    version: 1,
    created_at: 1_700_000_000_000_000,
    format: "openai_response",
    inference: {
        url: [
            {
                type: "base",
            },
            {
                type: "literal",
                value: "/responses",
            },
        ],
        upstream_streaming: "match_client",
    },
    credential: {
        type: "api_key",
    },
    discovery: {
        request: {
            method: "GET",
            url: [
                {
                    type: "base",
                },
                {
                    type: "literal",
                    value: "/models",
                },
            ],
        },
        extract: {
            mode: "array",
            array_at: [
                "data",
            ],
            id_field: [
                "id",
            ],
        },
        pagination: {
            type: "none",
        },
    },
};

describe("validateDefinitionPayload", () => {
    it("accepts every catalog payload verbatim", () => {
        for (const preset of PROVIDER_PRESETS) {
            const result = validateDefinitionPayload(preset.payload);

            if ("error" in result) {
                throw new Error(`${preset.id} was rejected: ${result.error}`);
            }

            expect(result.payload.format, preset.id).toBe(preset.payload.format);
        }
    });

    it("rejects a payload missing its format, naming the path", () => {
        const { format: _format, ...withoutFormat } = PROVIDER_PRESETS[0].payload;
        const result = validateDefinitionPayload(withoutFormat);

        if (!("error" in result)) {
            throw new Error("the payload should have been rejected");
        }

        expect(result.error).toMatch(/definition\.format is not an upstream format/);
    });

    it("rejects an upstream streaming mode outside its two-value union, naming the path", () => {
        const result = validateDefinitionPayload({
            ...PROVIDER_PRESETS[0].payload,
            inference: {
                ...PROVIDER_PRESETS[0].payload.inference,
                upstream_streaming: "sometimes",
            },
        });

        if (!("error" in result)) {
            throw new Error("the payload should have been rejected");
        }

        expect(result.error).toMatch(
            /definition\.inference\.upstream_streaming is not an upstream streaming mode/,
        );
    });

    it("rejects a json template leaf without a JSON value, naming the path", () => {
        const result = validateDefinitionPayload({
            ...PROVIDER_PRESETS[0].payload,
            inference: {
                ...PROVIDER_PRESETS[0].payload.inference,
                envelope: {
                    type: "json",
                },
            },
        });

        if (!("error" in result)) {
            throw new Error("the payload should have been rejected");
        }

        expect(result.error).toMatch(/definition\.inference\.envelope\.value is not a JSON value/);
    });

    it("rejects a field the descriptor does not declare, naming the path", () => {
        const result = validateDefinitionPayload({
            ...PROVIDER_PRESETS[0].payload,
            name: "generic",
        });

        if (!("error" in result)) {
            throw new Error("the payload should have been rejected");
        }

        expect(result.error).toMatch(/definition\.name is not a declared field/);
    });
});

describe("definition response validation", () => {
    it("parses the stored listing and the envelope, building every row", () => {
        expect(
            parseListProviderDefinitionsResponse({
                provider_definitions: [
                    storedDefinition,
                ],
            }),
        ).toEqual({
            provider_definitions: [
                storedDefinition,
            ],
        });
    });

    it("rejects a stored row missing a field the app reads", () => {
        const { version: _version, ...withoutVersion } = storedDefinition;

        expect(() =>
            parseListProviderDefinitionsResponse({
                provider_definitions: [
                    withoutVersion,
                ],
            }),
        ).toThrow(/response\.provider_definitions\[0\]\.version must be an integer/);
    });

    it("rejects a stored payload that breaks the descriptor contract", () => {
        expect(() =>
            parseListProviderDefinitionsResponse({
                provider_definitions: [
                    {
                        ...storedDefinition,
                        inference: {
                            ...storedDefinition.inference,
                            upstream_streaming: "sometimes",
                        },
                    },
                ],
            }),
        ).toThrow(
            /response\.provider_definitions\[0\]\.inference\.upstream_streaming is not an upstream streaming mode/,
        );

        expect(() =>
            parseListProviderDefinitionsResponse({
                provider_definitions: [
                    {
                        ...storedDefinition,
                        credential: {
                            type: "oauth",
                        },
                    },
                ],
            }),
        ).toThrow(/response\.provider_definitions\[0\]\.credential\.client_id must be a string/);
    });
});
