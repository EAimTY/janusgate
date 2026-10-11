import { describe, expect, test } from "vitest";
import { ContractViolationError } from "../errors";
import { parseDiscoverModelsResponse, parseListModelsResponse, parseModelEnvelope } from "./models";

/**
 * The model response parsers: the four discovery-owned capability bases
 * beside the four administrator-owned overrides, the reasoning levels
 * narrowed against the closed vocabulary, and the compaction-facts columns
 * split by what each may carry — the discovered base stores known facts
 * only, while the override states the explicit `"unknown"` too.
 */

/** One fully-populated model row fixture. */
const MODEL_ROW = {
    id: 31,
    provider_id: 2,
    provider: "acme",
    upstream_id: "claude-x",
    is_enabled: true,
    discovery_state: "discovered",
    thinking_levels: [
        "low",
        "high",
    ],
    max_input_tokens: 200_000,
    max_output_tokens: 8_192,
    thinking_levels_override: [
        "none",
    ],
    max_input_tokens_override: null,
    max_output_tokens_override: null,
    compaction_support: {
        responses_standalone_compact: "supported",
    },
    compaction_support_override: {
        responses_standalone_compact: "unknown",
    },
};

describe("model response parsers", () => {
    test("parse the listing with every capability column in place", () => {
        const listing = parseListModelsResponse({
            models: [
                MODEL_ROW,
            ],
        });

        const row = listing.models[0];
        if (row === undefined) throw new Error("the fixture listed no row");
        expect(row.thinking_levels_override).toEqual([
            "none",
        ]);
        expect(row.compaction_support).toEqual({
            responses_standalone_compact: "supported",
        });
        expect(row.compaction_support_override).toEqual({
            responses_standalone_compact: "unknown",
        });
    });

    test("parse discovery and envelope answers through the same row walk", () => {
        expect(
            parseDiscoverModelsResponse({
                models: [],
            }).models,
        ).toEqual([]);
        expect(
            parseModelEnvelope({
                model: MODEL_ROW,
            }).model.upstream_id,
        ).toBe("claude-x");
    });

    test("reject a foreign reasoning level — both write paths store vocabulary sets only", () => {
        expect(() =>
            parseListModelsResponse({
                models: [
                    {
                        ...MODEL_ROW,
                        thinking_levels: [
                            "extreme",
                        ],
                    },
                ],
            }),
        ).toThrowError("response.models[0].thinking_levels[0] is not a reasoning level: extreme");
        expect(() =>
            parseListModelsResponse({
                models: [
                    {
                        ...MODEL_ROW,
                        thinking_levels_override: [
                            "max",
                        ],
                    },
                ],
            }),
        ).not.toThrowError();
    });

    test("reject an unknown support fact in the discovered base but accept it in the override", () => {
        expect(() =>
            parseListModelsResponse({
                models: [
                    {
                        ...MODEL_ROW,
                        compaction_support: {
                            responses_explicit_trigger: "unknown",
                        },
                        compaction_support_override: null,
                    },
                ],
            }),
        ).toThrowError(ContractViolationError);

        expect(() =>
            parseListModelsResponse({
                models: [
                    {
                        ...MODEL_ROW,
                        compaction_support_override: {
                            messages_on_demand_compaction: "unknown",
                        },
                    },
                ],
            }),
        ).not.toThrowError();
    });

    test("reject a foreign mechanism key and a null model row", () => {
        expect(() =>
            parseListModelsResponse({
                models: [
                    {
                        ...MODEL_ROW,
                        compaction_support: {
                            not_a_mechanism: "supported",
                        },
                    },
                ],
            }),
        ).toThrowError("is not a compaction mechanism: not_a_mechanism");
        expect(() =>
            parseModelEnvelope({
                model: null,
            }),
        ).toThrowError("response.model must be an object");
    });

    test("keep a null capability base as the unknown state it is", () => {
        const listing = parseListModelsResponse({
            models: [
                {
                    ...MODEL_ROW,
                    thinking_levels: null,
                    max_input_tokens: null,
                    max_output_tokens: null,
                    compaction_support: null,
                },
            ],
        });

        const row = listing.models[0];
        if (row === undefined) throw new Error("the fixture listed no row");
        expect(row.thinking_levels).toBeNull();
        expect(row.max_input_tokens).toBeNull();
        expect(row.compaction_support).toBeNull();
    });
});
