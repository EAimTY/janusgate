import { describe, expect, it } from "vitest";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { indexRowsById } from "/src/row-index";
import {
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
