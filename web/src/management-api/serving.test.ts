import { describe, expect, it } from "vitest";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { isModelEffectivelyServing } from "./serving";

const provider: ProviderResponse = {
    type: "openai_chat_completion",
    id: 4,
    name: "local",
    model_prefix: "local-",
    is_enabled: true,
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
