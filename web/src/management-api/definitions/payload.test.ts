import { describe, expect, test } from "vitest";
import { PROVIDER_PRESETS } from "/src/presets/catalog";
import { validateDefinitionPayload } from "./payload";

/**
 * The authored definition payload's browser-side gate: the same strictness
 * the gateway's `deny_unknown_fields` descriptors apply, before the round
 * trip — every closed vocabulary checked against the generated types, every
 * required key's presence, and every undeclared key named at its path. The
 * shipped preset catalog doubles as the fixture corpus: every preset must
 * pass the gate the creation page holds its edited text to.
 */

describe("validateDefinitionPayload", () => {
    test("answers the typed payload for a well-formed static-key payload", () => {
        const result = validateDefinitionPayload({
            format: "openai_chat_completion",
            inference: {
                url: [
                    {
                        type: "literal",
                        value: "https://api.example.test/v1/chat/completions",
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "api_key",
            },
        });

        expect(result).toHaveProperty("payload");
        if ("payload" in result) {
            expect(result.payload.format).toBe("openai_chat_completion");
            expect(result.payload.credential).toEqual({
                type: "api_key",
            });
        }
    });

    test("names an undeclared key at its path instead of dropping it", () => {
        const result = validateDefinitionPayload({
            format: "openai_chat_completion",
            inference: {
                url: [
                    {
                        type: "literal",
                        value: "https://api.example.test/v1",
                    },
                ],
                upstream_streaming: "match_client",
                surprise: true,
            },
            credential: {
                type: "api_key",
            },
        });

        expect(result).toHaveProperty("error");
        if ("error" in result) {
            expect(result.error).toBe("definition.inference.surprise is not a declared field");
        }
    });

    test("names an undeclared top-level key and rejects a non-record payload", () => {
        expect(
            validateDefinitionPayload({
                format: "openai_chat_completion",
                credential: {
                    type: "api_key",
                },
                inference: {
                    url: [
                        {
                            type: "literal",
                            value: "https://api.example.test/v1",
                        },
                    ],
                    upstream_streaming: "match_client",
                },
                extra: 1,
            }),
        ).toHaveProperty("error", "definition.extra is not a declared field");

        expect(validateDefinitionPayload("nope")).toHaveProperty(
            "error",
            "definition must be an object",
        );
    });

    test("requires the inference URL to carry at least one expression part", () => {
        expect(
            validateDefinitionPayload({
                format: "openai_chat_completion",
                inference: {
                    url: [],
                    upstream_streaming: "match_client",
                },
                credential: {
                    type: "api_key",
                },
            }),
        ).toHaveProperty(
            "error",
            "definition.inference.url must carry at least one expression part",
        );
    });

    test("rejects a render option outside the definition format's closed set", () => {
        const result = validateDefinitionPayload({
            format: "openai_chat_completion",
            inference: {
                url: [
                    {
                        type: "literal",
                        value: "https://api.example.test/v1",
                    },
                ],
                upstream_streaming: "match_client",
                render_options: [
                    "omit_max_output_tokens",
                ],
            },
            credential: {
                type: "api_key",
            },
        });

        expect("error" in result && result.error).toContain(
            "is not part of the `openai_chat_completion` format's set",
        );
    });

    test("requires the envelope to carry exactly one body placeholder", () => {
        const noBody = validateDefinitionPayload({
            format: "openai_chat_completion",
            inference: {
                url: [
                    {
                        type: "literal",
                        value: "https://api.example.test/v1",
                    },
                ],
                upstream_streaming: "always",
                envelope: {
                    type: "object",
                    fields: {},
                },
            },
            credential: {
                type: "api_key",
            },
        });

        expect("error" in noBody && noBody.error).toContain(
            "carries 0 body placeholders; exactly one is required",
        );
    });

    test("rejects provisioning that rides a static-key credential or carries no step", () => {
        const inference = {
            url: [
                {
                    type: "literal",
                    value: "https://api.example.test/v1",
                },
            ],
            upstream_streaming: "match_client",
        } as const;

        const staticKey = validateDefinitionPayload({
            format: "openai_chat_completion",
            inference,
            credential: {
                type: "api_key",
            },
            provisioning: [],
        });

        expect("error" in staticKey && staticKey.error).toBe(
            "definition.provisioning must carry at least one step when present",
        );

        const oneStep = validateDefinitionPayload({
            format: "openai_chat_completion",
            inference,
            credential: {
                type: "api_key",
            },
            provisioning: [
                {
                    request: {
                        method: "GET",
                        url: [
                            {
                                type: "literal",
                                value: "https://api.example.test/me",
                            },
                        ],
                    },
                    response_encoding: "json",
                },
            ],
        });

        expect("error" in oneStep && oneStep.error).toContain(
            "provisioning requires an OAuth credential",
        );
    });

    test("keeps the payload sections the payload omits absent", () => {
        const result = validateDefinitionPayload({
            format: "openai_chat_completion",
            inference: {
                url: [
                    {
                        type: "literal",
                        value: "https://api.example.test/v1",
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "api_key",
            },
        });

        if ("payload" in result) {
            expect(result.payload.provisioning).toBeUndefined();
            expect(result.payload.discovery).toBeUndefined();
            expect(result.payload.compaction).toBeUndefined();
            expect(result.payload.quota).toBeUndefined();
        }
    });
});

describe("preset catalog against the payload gate", () => {
    test("passes every shipped preset through the authored-payload validator", () => {
        expect(PROVIDER_PRESETS.length).toBeGreaterThan(0);

        for (const preset of PROVIDER_PRESETS) {
            const result = validateDefinitionPayload(preset.payload);
            expect(`preset ${preset.id}: ${"error" in result ? result.error : ""}`).toBe(
                `preset ${preset.id}: `,
            );
        }
    });

    test("keeps every preset id unique", () => {
        const ids = PROVIDER_PRESETS.map((preset) => preset.id);

        expect(new Set(ids).size).toBe(ids.length);
    });

    test("carries the grant only where the payload's credential is OAuth", () => {
        for (const preset of PROVIDER_PRESETS) {
            const isOAuth = preset.payload.credential.type === "oauth";
            expect(`${preset.id}:${isOAuth}:${preset.grant === null}`).toBe(
                `${preset.id}:${isOAuth}:${!isOAuth}`,
            );
        }
    });
});
