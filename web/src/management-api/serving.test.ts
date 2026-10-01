import { describe, expect, it } from "vitest";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { indexRowsById } from "/src/row-index";
import {
    capabilitySource,
    effectiveMaxInputTokens,
    effectiveMaxOutputTokens,
    effectiveThinkingLevels,
    isModelEffectivelyServing,
    modelCountsByProvider,
    servingModelCountsByProvider,
} from "./serving";

const provider: ProviderResponse = {
    type: "api_key",
    id: 4,
    name: "local",
    model_prefix: "local-",
    is_enabled: true,
    provider_definition_name: "generic-openai-chat-completions",
    provider_definition_version: 1,
    format: "openai_chat_completion",
    base_url: "https://upstream.example.com/v1",
};

const disabledProvider: ProviderResponse = {
    ...provider,
    is_enabled: false,
};

function model(overrides: Partial<ModelResponse> = {}): ModelResponse {
    return {
        id: 9,
        provider_id: 4,
        provider: "local",
        upstream_id: "qwen3:32b",
        public_id: "local-qwen3-32b",
        id_override: null,
        is_enabled: true,
        discovery_state: "discovered",
        thinking_levels: null,
        max_input_tokens: null,
        max_output_tokens: null,
        thinking_levels_override: null,
        max_input_tokens_override: null,
        max_output_tokens_override: null,
        ...overrides,
    };
}

describe("isModelEffectivelyServing", () => {
    it("serves only when the provider, the row, and discovery all agree", () => {
        expect(isModelEffectivelyServing(model(), provider)).toBe(true);
    });

    it("is false when the row's own switch is off", () => {
        expect(
            isModelEffectivelyServing(
                model({
                    is_enabled: false,
                }),
                provider,
            ),
        ).toBe(false);
    });

    it("is false when the provider is off, without touching the row", () => {
        expect(isModelEffectivelyServing(model(), disabledProvider)).toBe(false);
    });

    it("is false for a vanished row even with both switches on", () => {
        expect(
            isModelEffectivelyServing(
                model({
                    discovery_state: "vanished",
                }),
                provider,
            ),
        ).toBe(false);
    });

    it("holds for a manual row with both switches on", () => {
        expect(
            isModelEffectivelyServing(
                model({
                    discovery_state: "manual",
                }),
                provider,
            ),
        ).toBe(true);
    });

    it("is false when the provider row is not at hand", () => {
        expect(isModelEffectivelyServing(model(), undefined)).toBe(false);
    });
});

describe("modelCountsByProvider", () => {
    const listed = [
        model(),
        model({
            id: 10,
            upstream_id: "qwen3:14b",
            discovery_state: "vanished",
        }),
    ];

    it("counts every listed row per provider when no predicate narrows it", () => {
        expect(modelCountsByProvider(listed).get(provider.id)).toBe(2);
    });

    it("counts only the rows the optional predicate includes", () => {
        const counts = modelCountsByProvider(
            listed,
            (candidate) => candidate.discovery_state !== "vanished",
        );

        expect(counts.get(provider.id)).toBe(1);
    });
});

describe("servingModelCountsByProvider", () => {
    it("counts only the rows that serve under the three-input rule", () => {
        const counts = servingModelCountsByProvider(
            [
                model(),
                model({
                    id: 10,
                    upstream_id: "qwen3:14b",
                    discovery_state: "vanished",
                }),
                model({
                    id: 11,
                    upstream_id: "qwen3:8b",
                    is_enabled: false,
                }),
            ],
            indexRowsById([
                provider,
            ]),
        );

        expect(counts.get(provider.id)).toBe(1);
    });

    it("excludes a disabled provider's rows without naming the provider a zero row", () => {
        // The count is per provider of the models it holds that serve: a
        // provider absent from the map has nothing counted, which is the
        // same answer a provider with only non-serving rows gets — the
        // caller states the attention, not the map.
        const counts = servingModelCountsByProvider(
            [
                model(),
                model({
                    id: 10,
                    upstream_id: "qwen3:14b",
                }),
            ],
            indexRowsById([
                disabledProvider,
            ]),
        );

        expect(counts.size).toBe(0);
    });

    it("excludes rows whose provider is not at hand", () => {
        const counts = servingModelCountsByProvider(
            [
                model(),
            ],
            indexRowsById([]),
        );

        expect(counts.size).toBe(0);
    });
});

describe("effective capability declarations", () => {
    it("reads the discovered set while no override stands", () => {
        const row = model({
            thinking_levels: [
                "low",
                "high",
            ],
            max_input_tokens: 200_000,
            max_output_tokens: 32_768,
        });

        expect(effectiveThinkingLevels(row)).toEqual([
            "low",
            "high",
        ]);
        expect(effectiveMaxInputTokens(row)).toBe(200_000);
        expect(effectiveMaxOutputTokens(row)).toBe(32_768);
        expect(capabilitySource(row.thinking_levels_override, row.thinking_levels)).toBe(
            "discovered",
        );
    });

    it("prefers the override wholesale, including the empty no-levels set", () => {
        const row = model({
            thinking_levels: [
                "low",
                "high",
            ],
            max_input_tokens: 200_000,
            thinking_levels_override: [],
            max_input_tokens_override: 100_000,
        });

        expect(effectiveThinkingLevels(row)).toEqual([]);
        expect(effectiveMaxInputTokens(row)).toBe(100_000);
        expect(effectiveMaxOutputTokens(row)).toBe(null);
        expect(capabilitySource(row.thinking_levels_override, row.thinking_levels)).toBe(
            "override",
        );
    });

    it("states unknown while neither the base nor the override is known", () => {
        const row = model();

        expect(effectiveThinkingLevels(row)).toBe(null);
        expect(effectiveMaxInputTokens(row)).toBe(null);
        expect(effectiveMaxOutputTokens(row)).toBe(null);
        expect(capabilitySource(row.thinking_levels_override, row.thinking_levels)).toBe("unknown");
    });
});
