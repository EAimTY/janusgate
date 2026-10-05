import { describe, expect, it } from "vitest";
import { hasBaseReference } from "../management-api/definitions/base-reference";
import {
    parseListProviderDefinitionsResponse,
    validateDefinitionPayload,
} from "../management-api/definitions/payload";
import { parseGetProviderQuotaResponse } from "../management-api/responses/quota";
import antigravity from "../quota-fixtures/antigravity.json?raw";
import claude from "../quota-fixtures/claude-code.json?raw";
import codex from "../quota-fixtures/codex.json?raw";
import copilot from "../quota-fixtures/github-copilot.json?raw";
import kimi from "../quota-fixtures/kimi-code.json?raw";
import { PROVIDER_PRESETS } from "./catalog";

describe("researched quota definition round trips", () => {
    it("keeps reviewed fixtures tied to catalog recipes through editing, storage decoding, and result decoding", () => {
        for (const [id, document] of [
            [
                "codex",
                codex,
            ],
            [
                "claude-code",
                claude,
            ],
            [
                "kimi-code",
                kimi,
            ],
            [
                "github-copilot",
                copilot,
            ],
            [
                "antigravity",
                antigravity,
            ],
        ]) {
            const preset = PROVIDER_PRESETS.find((value) => value.id === id);
            if (preset === undefined) throw new Error(`missing fixture preset ${id}`);
            const fixture: unknown = JSON.parse(document ?? "");
            if (typeof fixture !== "object" || fixture === null)
                throw new Error("fixture must be an object");
            const fixtureDefinition = validateDefinitionPayload({
                ...preset.payload,
                quota: Reflect.get(fixture, "descriptor"),
            });
            if ("error" in fixtureDefinition) throw new Error(fixtureDefinition.error);
            expect(preset.payload.quota).toEqual(fixtureDefinition.payload.quota);
            const edited = validateDefinitionPayload({
                ...preset.payload,
                inference: {
                    ...preset.payload.inference,
                    url: [
                        ...preset.payload.inference.url,
                        {
                            type: "literal",
                            value: "?edited",
                        },
                    ],
                },
            });
            if ("error" in edited) throw new Error(edited.error);
            const stored = parseListProviderDefinitionsResponse({
                provider_definitions: [
                    {
                        ...edited.payload,
                        name: id,
                        version: 2,
                        created_at: 1,
                    },
                ],
            }).provider_definitions[0];
            expect(stored?.quota).toEqual(preset.payload.quota);
            expect(
                parseGetProviderQuotaResponse({
                    observed_at: 1,
                    quotas: Reflect.get(fixture, "expected"),
                }).quotas,
            ).toEqual(Reflect.get(fixture, "expected"));
        }
    });

    it("recognizes base references in every quota request leaf and rejects unavailable stage values", () => {
        const preset = PROVIDER_PRESETS.find((value) => value.id === "claude-code");
        if (preset?.payload.quota === undefined || preset.payload.quota === null)
            throw new Error("fixture quota missing");
        for (const request of [
            {
                method: "GET",
                url: [
                    {
                        type: "base",
                    },
                ],
            },
            {
                method: "GET",
                url: [
                    {
                        type: "literal",
                        value: "https://example.test",
                    },
                ],
                headers: [
                    {
                        name: "x-base",
                        value: [
                            {
                                type: "base",
                            },
                        ],
                    },
                ],
            },
            {
                method: "GET",
                url: [
                    {
                        type: "literal",
                        value: "https://example.test",
                    },
                ],
                query: [
                    {
                        name: "base",
                        value: [
                            {
                                type: "base",
                            },
                        ],
                    },
                ],
            },
            {
                method: "POST",
                url: [
                    {
                        type: "literal",
                        value: "https://example.test",
                    },
                ],
                body: {
                    kind: "form",
                    fields: [
                        {
                            name: "base",
                            value: [
                                {
                                    type: "base",
                                },
                            ],
                        },
                    ],
                },
            },
            {
                method: "POST",
                url: [
                    {
                        type: "literal",
                        value: "https://example.test",
                    },
                ],
                body: {
                    kind: "json",
                    node: {
                        type: "object",
                        fields: {
                            nested: {
                                type: "array",
                                items: [
                                    {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "coalesce",
                                                parts: [
                                                    {
                                                        type: "base",
                                                    },
                                                    {
                                                        type: "literal",
                                                        value: "fallback",
                                                    },
                                                ],
                                            },
                                        ],
                                    },
                                ],
                            },
                        },
                    },
                },
            },
        ]) {
            const result = validateDefinitionPayload({
                ...preset.payload,
                quota: {
                    ...preset.payload.quota,
                    request,
                },
            });
            if ("error" in result) throw new Error(result.error);
            expect(hasBaseReference(result.payload)).toBe(true);
        }
        for (const part of [
            {
                type: "model",
            },
            {
                type: "request_uuid",
            },
            {
                type: "refresh_token",
            },
            {
                type: "client_id",
            },
            {
                type: "client_secret",
            },
            {
                type: "slot",
                name: "missing",
            },
        ]) {
            expect(
                validateDefinitionPayload({
                    ...preset.payload,
                    quota: {
                        ...preset.payload.quota,
                        request: {
                            ...preset.payload.quota.request,
                            query: [
                                {
                                    name: "bad",
                                    value: [
                                        part,
                                    ],
                                },
                            ],
                        },
                    },
                }),
            ).toHaveProperty("error");
        }
    });
});
