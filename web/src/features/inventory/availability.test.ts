import { describe, expect, test } from "vitest";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { classifyStructuralAvailability } from "./availability";

/**
 * The structural serving ladder for one account-model pair: the account
 * rungs stand first (existence, then switch), then the model's (existence,
 * switch), then the discovery state — independent of quota restrictions and
 * of any pool or route switch above the pair.
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

describe("classifyStructuralAvailability", () => {
    test("answers the account rungs before consulting the model row at all", () => {
        expect(classifyStructuralAvailability(undefined, model())).toBe("account_missing");
        expect(
            classifyStructuralAvailability(
                provider({
                    is_enabled: false,
                }),
                model({
                    is_enabled: false,
                }),
            ),
        ).toBe("account_disabled");
    });

    test("answers the model rungs once the account stands enabled", () => {
        const enabled = provider();

        expect(classifyStructuralAvailability(enabled, undefined)).toBe("model_missing");
        expect(
            classifyStructuralAvailability(
                enabled,
                model({
                    is_enabled: false,
                }),
            ),
        ).toBe("model_disabled");
    });

    test("answers the ladder's terminal rung through the contract's own readiness verdict", () => {
        const enabled = provider();

        expect(classifyStructuralAvailability(enabled, model())).toBe("ready");
        expect(
            classifyStructuralAvailability(
                enabled,
                model({
                    discovery_state: "vanished",
                }),
            ),
        ).toBe("model_vanished");
        // A manual row is standing inventory, not a vanished one.
        expect(
            classifyStructuralAvailability(
                enabled,
                model({
                    discovery_state: "manual",
                }),
            ),
        ).toBe("ready");
    });
});
