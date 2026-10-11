import { describe, expect, test } from "vitest";
import type {
    GetProviderAvailabilityResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { deriveMemberAvailability } from "../provider-pools/pool-availability";

/**
 * The pool member's composite availability: the pool's own two rungs (the
 * authored switch and the exact member choice) standing beside the shared
 * structural ladder — the account rungs outrank the member choice — with the
 * observation's applicable blocks filtered by the selected upstream id and a
 * read-only join never clearing a block because its reset time passed.
 */

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
        provider_definition_name: "generic-openai-chat-completions",
        provider_definition_version: 1,
        format: "openai_chat_completion",
        base_url: null,
        ...overrides,
    };
}

function model(overrides: Partial<ModelResponse> = {}): ModelResponse {
    return {
        id: 10,
        provider_id: 1,
        provider: "acme",
        upstream_id: "gpt-x",
        is_enabled: true,
        discovery_state: "discovered",
        thinking_levels: null,
        max_input_tokens: null,
        max_output_tokens: null,
        thinking_levels_override: null,
        max_input_tokens_override: null,
        max_output_tokens_override: null,
        compaction_support: null,
        compaction_support_override: null,
        ...overrides,
    };
}

/** One availability observation carrying an all-models block and a model-scoped one. */
function observation(): GetProviderAvailabilityResponse {
    return {
        blocks: [
            {
                key: [
                    "daily",
                ],
                applies_to: {
                    type: "all_models",
                },
                observed_at: 1000,
                blocked_until: 2000,
            },
            {
                key: [
                    "scoped",
                ],
                applies_to: {
                    type: "models",
                    upstream_ids: [
                        "gpt-x",
                    ],
                },
                observed_at: 1000,
                blocked_until: 3000,
            },
            {
                key: [
                    "other",
                ],
                applies_to: {
                    type: "models",
                    upstream_ids: [
                        "other-model",
                    ],
                },
                observed_at: 1000,
                blocked_until: 4000,
            },
        ],
        last_refresh: {
            type: "not_observed",
        },
    };
}

describe("deriveMemberAvailability", () => {
    test("answers the pool's own disabled switch above every other rung", () => {
        const result = deriveMemberAvailability(
            false,
            provider(),
            [
                model(),
            ],
            "gpt-x",
            null,
        );

        expect(result.inventory).toBe("pool_disabled");
    });

    test("outranks the member choice with the account rungs", () => {
        expect(
            deriveMemberAvailability(
                true,
                undefined,
                [
                    model(),
                ],
                "gpt-x",
                null,
            ).inventory,
        ).toBe("account_missing");
        expect(
            deriveMemberAvailability(
                true,
                provider({
                    is_enabled: false,
                }),
                [
                    model(),
                ],
                "gpt-x",
                null,
            ).inventory,
        ).toBe("account_disabled");
    });

    test("answers model_not_selected while no exact member choice stands", () => {
        expect(
            deriveMemberAvailability(
                true,
                provider(),
                [
                    model(),
                ],
                null,
                null,
            ).inventory,
        ).toBe("model_not_selected");
    });

    test("walks the shared ladder below the member choice", () => {
        expect(
            deriveMemberAvailability(
                true,
                provider(),
                [
                    model(),
                ],
                "gpt-x",
                null,
            ).inventory,
        ).toBe("ready");
        expect(deriveMemberAvailability(true, provider(), [], "gpt-x", null).inventory).toBe(
            "model_missing",
        );
        expect(
            deriveMemberAvailability(
                true,
                provider(),
                [
                    model({
                        is_enabled: false,
                    }),
                ],
                "gpt-x",
                null,
            ).inventory,
        ).toBe("model_disabled");
        expect(
            deriveMemberAvailability(
                true,
                provider(),
                [
                    model({
                        discovery_state: "vanished",
                    }),
                ],
                "gpt-x",
                null,
            ).inventory,
        ).toBe("model_vanished");
    });

    test("keeps the applicable blocks by the selected upstream id, the expired ones included", () => {
        const result = deriveMemberAvailability(
            true,
            provider(),
            [
                model(),
            ],
            "gpt-x",
            observation(),
        );

        // The all-models block and the gpt-x-scoped block apply; the long-expired
        // one is never cleared by the read-only join.
        expect(result.blocks.map((block) => block.key[0])).toEqual([
            "daily",
            "scoped",
        ]);
        expect(result.refresh).toEqual({
            type: "not_observed",
        });
    });

    test("keeps only the all-models blocks when no member is selected", () => {
        const result = deriveMemberAvailability(
            true,
            provider(),
            [
                model(),
            ],
            null,
            observation(),
        );

        expect(result.blocks.map((block) => block.key[0])).toEqual([
            "daily",
        ]);
    });

    test("answers no blocks and no refresh without an observation", () => {
        const result = deriveMemberAvailability(
            true,
            provider(),
            [
                model(),
            ],
            "gpt-x",
            null,
        );

        expect(result.blocks).toEqual([]);
        expect(result.refresh).toBeNull();
    });
});
